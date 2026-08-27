//! The property registry: one declarative row per GTK 4.22 CSS property.
//!
//! Nothing outside this module names a property string. `Prop`'s
//! discriminant is the `PROPERTIES` index and, for longhands, the
//! `ComputedStyle` slot, so the enum's declaration order is load-bearing:
//! rows may only be appended to the end of their own group.

// Every registry parser returns `Result<Value, ()>`: the contract freezes
// that signature, and `Err(())` carries all the meaning CSS gives it --
// *invalid at parse time*, drop the declaration.
#![allow(clippy::result_unit_err)]

use std::rc::Rc;

use cssparser::{Parser, ParserInput};

use crate::css::value::Value;
use crate::css::value::border::{
    BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle, parse_line_style,
    parse_line_width,
};
use crate::css::value::color::{ColorValue, Rgba};
use crate::css::value::filter::parse_filter_list;
use crate::css::value::font::{
    FontFamily, FontStyle, FontVariantFlags, FontWeight, GenericFamily, LineHeight,
    parse_family_list, parse_feature_settings, parse_font_size, parse_stretch, parse_variant_flags,
    parse_variation_settings,
};
use crate::css::value::image::{Image, Position, parse_icon_palette};
use crate::css::value::interpolate;
use crate::css::value::keyword::Keyword;
use crate::css::value::length::Length;
use crate::css::value::shadow::Shadow;
use crate::css::value::shorthand as sh_fns;
use crate::css::value::text::parse_decoration_lines;
use crate::css::value::timing::{AnimationName, IterationCount, Time, TimingFunction};
use crate::css::value::transform::parse_transform_list;

/// Whole-value parser. Consumes the entire input; a trailing token is an
/// error. Never panics. `Err(())` == invalid at parse time, so the
/// declaration is dropped, CSS-style.
pub type ParseFn = fn(&mut cssparser::Parser<'_, '_>) -> Result<Value, ()>;

/// Expands a shorthand, emitting one `(longhand, Value)` per longhand it
/// sets. MUST emit every longhand in `PropertyKind::Shorthand::longhands`:
/// omitted components are emitted at their initial value (CSS's shorthand
/// reset rule).
pub type ExpandFn =
    fn(&mut cssparser::Parser<'_, '_>, &mut dyn FnMut(Prop, Value)) -> Result<(), ()>;

/// Interpolate two *computed* values of the same property at progress `t`
/// (usually `0..=1`; a cubic-bezier may overshoot).
pub type Interpolate = fn(&Value, &Value, f32) -> Value;

/// What a registry row is.
pub enum PropertyKind {
    /// A property with its own computed slot.
    Longhand {
        /// Whole-value parser.
        parse: ParseFn,
        /// Initial value constructor.
        initial: fn() -> Value,
        /// Whether the property inherits.
        inherited: bool,
        /// `Some` for animatable properties.
        animatable: Option<Interpolate>,
    },
    /// A property that only sets other properties.
    Shorthand {
        /// Expansion function.
        expand: ExpandFn,
        /// The longhands it resets, in reset order.
        longhands: &'static [Prop],
    },
}

/// One registry row.
pub struct PropertyDef {
    /// The property's CSS name, ASCII-lowercase.
    pub name: &'static str,
    /// Longhand or shorthand.
    pub kind: PropertyKind,
}

/// Declares `Prop`, `Prop::ALL` and the name table from one list, so a
/// discriminant and its name can never drift apart.
macro_rules! props {
    (longhands { $( $lh:ident => $lname:literal ),+ $(,)? }
     shorthands { $( $sh:ident => $sname:literal ),+ $(,)? }) => {
        /// Every property in the GTK 4.22 CSS reference.
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub enum Prop { $( $lh, )+ $( $sh, )+ }

        /// Number of longhands; also the `ComputedStyle` slot count.
        pub const N_LONGHANDS: usize = [$( stringify!($lh) ),+].len();
        /// Number of rows in [`PROPERTIES`].
        pub const N_PROPS: usize = N_LONGHANDS + [$( stringify!($sh) ),+].len();

        impl Prop {
            /// Every property, in registry order.
            pub const ALL: &'static [Prop] = &[ $( Prop::$lh, )+ $( Prop::$sh, )+ ];

            /// The property's CSS name.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self { $( Prop::$lh => $lname, )+ $( Prop::$sh => $sname, )+ }
            }
        }
    };
}

props! {
    longhands {
        // colours & effects
        Color => "color", Opacity => "opacity", Filter => "filter",
        // fonts
        FontFamily => "font-family", FontSize => "font-size",
        FontStyle => "font-style", FontVariant => "font-variant",
        FontWeight => "font-weight", FontWidth => "font-width",
        FontStretch => "font-stretch", FontKerning => "font-kerning",
        FontVariantLigatures => "font-variant-ligatures",
        FontVariantPosition => "font-variant-position",
        FontVariantCaps => "font-variant-caps",
        FontVariantNumeric => "font-variant-numeric",
        FontVariantAlternates => "font-variant-alternates",
        FontVariantEastAsian => "font-variant-east-asian",
        FontFeatureSettings => "font-feature-settings",
        FontVariationSettings => "font-variation-settings",
        GtkDpi => "-gtk-dpi",
        // text
        CaretColor => "caret-color",
        GtkSecondaryCaretColor => "-gtk-secondary-caret-color",
        LetterSpacing => "letter-spacing", TextTransform => "text-transform",
        LineHeight => "line-height",
        TextDecorationLine => "text-decoration-line",
        TextDecorationColor => "text-decoration-color",
        TextDecorationStyle => "text-decoration-style",
        TextShadow => "text-shadow",
        // icons (parsed + stored; drawn in M4)
        GtkIconSource => "-gtk-icon-source", GtkIconSize => "-gtk-icon-size",
        GtkIconStyle => "-gtk-icon-style", GtkIconTransform => "-gtk-icon-transform",
        GtkIconPalette => "-gtk-icon-palette", GtkIconShadow => "-gtk-icon-shadow",
        GtkIconFilter => "-gtk-icon-filter", GtkIconWeight => "-gtk-icon-weight",
        // transform
        Transform => "transform", TransformOrigin => "transform-origin",
        // box model
        MinWidth => "min-width", MinHeight => "min-height",
        MarginTop => "margin-top", MarginRight => "margin-right",
        MarginBottom => "margin-bottom", MarginLeft => "margin-left",
        PaddingTop => "padding-top", PaddingRight => "padding-right",
        PaddingBottom => "padding-bottom", PaddingLeft => "padding-left",
        // borders
        BorderTopWidth => "border-top-width", BorderRightWidth => "border-right-width",
        BorderBottomWidth => "border-bottom-width", BorderLeftWidth => "border-left-width",
        BorderTopStyle => "border-top-style", BorderRightStyle => "border-right-style",
        BorderBottomStyle => "border-bottom-style", BorderLeftStyle => "border-left-style",
        BorderTopLeftRadius => "border-top-left-radius",
        BorderTopRightRadius => "border-top-right-radius",
        BorderBottomRightRadius => "border-bottom-right-radius",
        BorderBottomLeftRadius => "border-bottom-left-radius",
        BorderTopColor => "border-top-color", BorderRightColor => "border-right-color",
        BorderBottomColor => "border-bottom-color", BorderLeftColor => "border-left-color",
        BorderImageSource => "border-image-source", BorderImageRepeat => "border-image-repeat",
        BorderImageSlice => "border-image-slice", BorderImageWidth => "border-image-width",
        // outline
        OutlineStyle => "outline-style", OutlineWidth => "outline-width",
        OutlineColor => "outline-color", OutlineOffset => "outline-offset",
        // backgrounds
        BackgroundColor => "background-color", BackgroundClip => "background-clip",
        BackgroundOrigin => "background-origin", BackgroundSize => "background-size",
        BackgroundPosition => "background-position", BackgroundRepeat => "background-repeat",
        BackgroundImage => "background-image", BoxShadow => "box-shadow",
        BackgroundBlendMode => "background-blend-mode",
        // transitions
        TransitionProperty => "transition-property",
        TransitionDuration => "transition-duration",
        TransitionTimingFunction => "transition-timing-function",
        TransitionDelay => "transition-delay",
        // animations
        AnimationName => "animation-name", AnimationDuration => "animation-duration",
        AnimationTimingFunction => "animation-timing-function",
        AnimationIterationCount => "animation-iteration-count",
        AnimationDirection => "animation-direction",
        AnimationPlayState => "animation-play-state",
        AnimationDelay => "animation-delay", AnimationFillMode => "animation-fill-mode",
        // misc
        BorderSpacing => "border-spacing",
    }
    shorthands {
        Font => "font", TextDecoration => "text-decoration",
        Margin => "margin", Padding => "padding",
        BorderWidth => "border-width", BorderStyle => "border-style",
        BorderColor => "border-color",
        BorderTop => "border-top", BorderRight => "border-right",
        BorderBottom => "border-bottom", BorderLeft => "border-left",
        Border => "border", BorderRadius => "border-radius",
        BorderImage => "border-image",
        Outline => "outline", Background => "background",
        Transition => "transition", Animation => "animation",
    }
}

impl Prop {
    /// Whether this row has its own computed slot.
    #[must_use]
    pub fn is_longhand(self) -> bool {
        (self as usize) < N_LONGHANDS
    }

    /// The `ComputedStyle` slot index. Longhands only.
    #[must_use]
    pub fn slot(self) -> usize {
        debug_assert!(self.is_longhand(), "{} is a shorthand", self.name());
        self as usize
    }
}

/// ASCII-case-insensitive property lookup; `None` for an unknown name.
#[must_use]
pub fn lookup(name: &str) -> Option<Prop> {
    if !name.is_ascii() {
        return None;
    }
    Prop::ALL
        .iter()
        .copied()
        .find(|prop| name.eq_ignore_ascii_case(prop.name()))
}

/// All longhands, in registry order -- the `ComputedStyle` slot order.
pub fn longhands() -> impl Iterator<Item = Prop> {
    Prop::ALL.iter().copied().take(N_LONGHANDS)
}

// ---- the table --------------------------------------------------------

const fn lh(
    name: &'static str,
    parse: ParseFn,
    initial: fn() -> Value,
    inherited: bool,
    animatable: Option<Interpolate>,
) -> PropertyDef {
    PropertyDef {
        name,
        kind: PropertyKind::Longhand {
            parse,
            initial,
            inherited,
            animatable,
        },
    }
}

const fn sh(name: &'static str, expand: ExpandFn, longhands: &'static [Prop]) -> PropertyDef {
    PropertyDef {
        name,
        kind: PropertyKind::Shorthand { expand, longhands },
    }
}

/// Every registry parser goes through the wide keywords first.
fn wide(input: &mut Parser<'_, '_>, inner: ParseFn) -> Result<Value, ()> {
    Value::parse_wide_or(input, inner)
}

fn keyword_in(input: &mut Parser<'_, '_>, allowed: &[Keyword]) -> Result<Keyword, ()> {
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    allowed
        .iter()
        .copied()
        .find(|keyword| name.eq_ignore_ascii_case(keyword.as_str()))
        .ok_or(())
}

// ---- longhand parsers -------------------------------------------------

fn p_color(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Color(ColorValue::parse(i)?)))
}

fn p_caret_color(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident()
            && name.eq_ignore_ascii_case("auto")
        {
            return Ok(Value::Color(ColorValue::CurrentColor));
        }
        i.reset(&state);
        Ok(Value::Color(ColorValue::parse(i)?))
    })
}

fn p_opacity(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let value = i.expect_number().map_err(|_| ())?;
        if !value.is_finite() {
            return Err(());
        }
        Ok(Value::Number(value.clamp(0.0, 1.0)))
    })
}

fn p_number(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let value = i.expect_number().map_err(|_| ())?;
        if value.is_finite() {
            Ok(Value::Number(value))
        } else {
            Err(())
        }
    })
}

fn p_length(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(Length::parse(i)?)))
}

fn p_length_auto(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Length(Length::parse_allowing_auto(i)?))
    })
}

fn p_line_width(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(parse_line_width(i)?)))
}

fn p_line_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Keyword(parse_line_style(i)?)))
}

/// A corner radius: `<length> <length>?`, the second defaulting to the first.
fn parse_length_pair(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    let horizontal = Length::parse(input)?;
    let state = input.state();
    let vertical = match Length::parse(input) {
        Ok(length) => length,
        Err(()) => {
            input.reset(&state);
            horizontal.clone()
        }
    };
    Ok(Value::Pair(Rc::new((
        Value::Length(horizontal),
        Value::Length(vertical),
    ))))
}

fn p_radius(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, parse_length_pair)
}

fn p_border_spacing(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, parse_length_pair)
}

fn p_image(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Image(Image::parse(i)?)))
}

fn p_image_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Image(Image::parse(item)?)))
    })
}

fn p_icon_source(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident()
            && name.eq_ignore_ascii_case("builtin")
        {
            return Ok(Value::Keyword(Keyword::Builtin));
        }
        i.reset(&state);
        Ok(Value::Image(Image::parse(i)?))
    })
}

fn p_icon_palette(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::IconPalette(parse_icon_palette(i)?)))
}

fn p_transform(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Transform(parse_transform_list(i)?)))
}

fn p_filter(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Filter(parse_filter_list(i)?)))
}

fn p_position(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Position(Position::parse(i)?)))
}

fn p_position_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Position(Position::parse(item)?)))
    })
}

fn p_bg_size_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::BgSize(BgSize::parse(item)?)))
    })
}

fn p_bg_repeat_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Repeat(RepeatStyle::parse_background(item)?))
        })
    })
}

fn p_border_image_repeat(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Repeat(RepeatStyle::parse(i)?)))
}

fn p_border_image_slice(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Slice(BorderImageSlice::parse(i)?)))
}

fn p_border_image_width(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let mut values: Vec<BorderImageWidthSide> = Vec::new();
        while values.len() < 5 {
            let state = i.state();
            match BorderImageWidthSide::parse(i) {
                Ok(value) => values.push(value),
                Err(()) => {
                    i.reset(&state);
                    break;
                }
            }
        }
        crate::css::value::border::four_sides(values)
            .map(Value::BorderImageWidths)
            .ok_or(())
    })
}

fn p_box_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::BorderBox,
                    Keyword::PaddingBox,
                    Keyword::ContentBox,
                    Keyword::TextBox,
                ],
            )?))
        })
    })
}

fn p_blend_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::Normal,
                    Keyword::Multiply,
                    Keyword::Screen,
                    Keyword::Overlay,
                    Keyword::Darken,
                    Keyword::Lighten,
                    Keyword::ColorDodge,
                    Keyword::ColorBurn,
                    Keyword::HardLight,
                    Keyword::SoftLight,
                    Keyword::Difference,
                    Keyword::Exclusion,
                    Keyword::Hue,
                    Keyword::Saturation,
                    Keyword::ColorBlend,
                    Keyword::Luminosity,
                ],
            )?))
        })
    })
}

fn p_shadow_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident()
            && name.eq_ignore_ascii_case("none")
        {
            return Ok(Value::List(Rc::from(Vec::new())));
        }
        i.reset(&state);
        Value::parse_list(i, |item| Ok(Value::Shadow(Shadow::parse(item)?)))
    })
}

fn p_text_shadow_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident()
            && name.eq_ignore_ascii_case("none")
        {
            return Ok(Value::List(Rc::from(Vec::new())));
        }
        i.reset(&state);
        Value::parse_list(i, |item| Ok(Value::Shadow(Shadow::parse_text(item)?)))
    })
}

fn p_family(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontFamilies(parse_family_list(i)?)))
}

fn p_font_size(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Length(parse_font_size(i)?)))
}

fn p_font_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontStyle(FontStyle::parse(i)?)))
}

fn p_font_weight(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::FontWeight(FontWeight::parse(i)?)))
}

fn p_stretch(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::Percentage(parse_stretch(i)? / 100.0)))
}

fn p_features(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::FontFeatures(parse_feature_settings(i)?))
    })
}

fn p_variations(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::FontVariations(parse_variation_settings(i)?))
    })
}

/// The `font-variant-*` longhands differ only in the flag subset they take.
macro_rules! variant_parser {
    ($name:ident, $( $flag:ident )|+) => {
        fn $name(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
            wide(input, |i| {
                const ALLOWED: FontVariantFlags =
                    FontVariantFlags::from_bits_truncate($( FontVariantFlags::$flag.bits() )|+);
                Ok(Value::FontVariant(parse_variant_flags(i, ALLOWED)?))
            })
        }
    };
}

// GTK supports only the CSS2 `font-variant` values (`normal | small-caps`).
variant_parser!(p_variant_css2, SMALL_CAPS);
variant_parser!(
    p_variant_ligatures,
    NO_COMMON_LIGATURES | DISCRETIONARY_LIGATURES | HISTORICAL_LIGATURES | NO_CONTEXTUAL
);
variant_parser!(p_variant_position, SUB | SUPER);
variant_parser!(
    p_variant_caps,
    SMALL_CAPS | ALL_SMALL_CAPS | PETITE_CAPS | ALL_PETITE_CAPS | UNICASE | TITLING_CAPS
);
variant_parser!(
    p_variant_numeric,
    LINING_NUMS
        | OLDSTYLE_NUMS
        | PROPORTIONAL_NUMS
        | TABULAR_NUMS
        | DIAGONAL_FRACTIONS
        | STACKED_FRACTIONS
        | ORDINAL
        | SLASHED_ZERO
);
variant_parser!(p_variant_alternates, HISTORICAL_FORMS);
variant_parser!(
    p_variant_east_asian,
    JIS78
        | JIS83
        | JIS90
        | JIS04
        | SIMPLIFIED
        | TRADITIONAL
        | FULL_WIDTH_EA
        | PROPORTIONAL_WIDTH_EA
        | RUBY
);

fn p_kerning(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[Keyword::Auto, Keyword::Normal, Keyword::None],
        )?))
    })
}

fn p_letter_spacing(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        let state = i.state();
        if let Ok(name) = i.expect_ident()
            && name.eq_ignore_ascii_case("normal")
        {
            return Ok(Value::Length(Length::zero()));
        }
        i.reset(&state);
        Ok(Value::Length(Length::parse(i)?))
    })
}

fn p_text_transform(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[
                Keyword::None,
                Keyword::Capitalize,
                Keyword::Uppercase,
                Keyword::Lowercase,
                Keyword::FullWidth,
                Keyword::FullSizeKana,
            ],
        )?))
    })
}

fn p_line_height(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| Ok(Value::LineHeight(LineHeight::parse(i)?)))
}

fn p_decoration_lines(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::TextDecorationLines(parse_decoration_lines(i)?))
    })
}

fn p_decoration_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[
                Keyword::Solid,
                Keyword::Double,
                Keyword::Dotted,
                Keyword::Dashed,
                Keyword::Wavy,
            ],
        )?))
    })
}

fn p_icon_style(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Ok(Value::Keyword(keyword_in(
            i,
            &[Keyword::Requested, Keyword::Regular, Keyword::Symbolic],
        )?))
    })
}

fn p_transition_property_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            let name = item.expect_ident().map_err(|_| ())?.as_ref().to_string();
            Ok(if name.eq_ignore_ascii_case("all") {
                Value::Keyword(Keyword::All)
            } else if name.eq_ignore_ascii_case("none") {
                Value::Keyword(Keyword::None)
            } else {
                Value::AnimationName(AnimationName::Named(Rc::from(name.as_str())))
            })
        })
    })
}

fn p_time_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Time(Time::parse(item)?)))
    })
}

fn p_timing_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| Ok(Value::Timing(TimingFunction::parse(item)?)))
    })
}

fn p_animation_name_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::AnimationName(AnimationName::parse(item)?))
        })
    })
}

fn p_iteration_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::IterationCount(IterationCount::parse(item)?))
        })
    })
}

fn p_direction_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::Normal,
                    Keyword::Reverse,
                    Keyword::Alternate,
                    Keyword::AlternateReverse,
                ],
            )?))
        })
    })
}

fn p_play_state_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[Keyword::Running, Keyword::Paused],
            )?))
        })
    })
}

fn p_fill_mode_list(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
    wide(input, |i| {
        Value::parse_list(i, |item| {
            Ok(Value::Keyword(keyword_in(
                item,
                &[
                    Keyword::None,
                    Keyword::Forwards,
                    Keyword::Backwards,
                    Keyword::Both,
                ],
            )?))
        })
    })
}

// ---- initial values ---------------------------------------------------

fn i_black() -> Value {
    Value::Color(ColorValue::Absolute(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    }))
}
fn i_transparent() -> Value {
    Value::Color(ColorValue::Absolute(Rgba::TRANSPARENT))
}
fn i_current_color() -> Value {
    Value::Color(ColorValue::CurrentColor)
}
fn i_one() -> Value {
    Value::Number(1.0)
}
fn i_dpi() -> Value {
    Value::Number(96.0)
}
fn i_weight_400() -> Value {
    Value::FontWeight(FontWeight::Absolute(400.0))
}
fn i_pct_one() -> Value {
    Value::Percentage(1.0)
}
fn i_zero_length() -> Value {
    Value::Length(Length::zero())
}
fn i_len_3() -> Value {
    Value::Length(Length::px(3.0))
}
fn i_len_14() -> Value {
    Value::Length(Length::px(14.0))
}
fn i_len_16() -> Value {
    Value::Length(Length::px(16.0))
}
fn i_none_keyword() -> Value {
    Value::Keyword(Keyword::None)
}
fn i_auto_keyword() -> Value {
    Value::Keyword(Keyword::Auto)
}
fn i_solid_keyword() -> Value {
    Value::Keyword(Keyword::Solid)
}
fn i_requested_keyword() -> Value {
    Value::Keyword(Keyword::Requested)
}
fn i_builtin_keyword() -> Value {
    Value::Keyword(Keyword::Builtin)
}
fn i_variant_empty() -> Value {
    Value::FontVariant(FontVariantFlags::empty())
}
fn i_features_empty() -> Value {
    Value::FontFeatures(Rc::from(Vec::new()))
}
fn i_variations_empty() -> Value {
    Value::FontVariations(Rc::from(Vec::new()))
}
fn i_family_sans() -> Value {
    Value::FontFamilies(Rc::from(vec![FontFamily::Generic(
        GenericFamily::SansSerif,
    )]))
}
fn i_style_normal() -> Value {
    Value::FontStyle(FontStyle::Normal)
}
fn i_line_height_normal() -> Value {
    Value::LineHeight(LineHeight::Normal)
}
fn i_decoration_none() -> Value {
    Value::TextDecorationLines(crate::css::value::text::TextDecorationLines::empty())
}
fn i_transform_none() -> Value {
    Value::Transform(Rc::from(Vec::new()))
}
fn i_filter_none() -> Value {
    Value::Filter(Rc::from(Vec::new()))
}
fn i_position_center() -> Value {
    Value::Position(Position::center())
}
fn i_radius_zero() -> Value {
    Value::Pair(Rc::new((i_zero_length(), i_zero_length())))
}
fn i_spacing_zero() -> Value {
    Value::Pair(Rc::new((i_zero_length(), i_zero_length())))
}
fn i_image_none() -> Value {
    Value::Image(Image::None)
}
fn i_empty_list() -> Value {
    Value::List(Rc::from(Vec::new()))
}
fn i_image_none_list() -> Value {
    Value::List(Rc::from(vec![Value::Image(Image::None)]))
}
fn i_position_origin_list() -> Value {
    Value::List(Rc::from(vec![Value::Position(Position {
        x: Length::Percent(0.0),
        y: Length::Percent(0.0),
        z: None,
    })]))
}
fn i_bg_size_auto_list() -> Value {
    Value::List(Rc::from(vec![Value::BgSize(BgSize::Auto)]))
}
fn i_bg_repeat_list() -> Value {
    Value::List(Rc::from(vec![Value::Repeat(RepeatStyle {
        x: Keyword::Repeat,
        y: Keyword::Repeat,
    })]))
}
fn i_clip_border_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::BorderBox)]))
}
fn i_origin_padding_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::PaddingBox)]))
}
fn i_blend_normal_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::Normal)]))
}
fn i_repeat_stretch() -> Value {
    Value::Repeat(RepeatStyle {
        x: Keyword::Stretch,
        y: Keyword::Stretch,
    })
}
fn i_slice_full() -> Value {
    Value::Slice(BorderImageSlice {
        sides: [NumberOrPercent::Percent(1.0); 4],
        fill: false,
    })
}
fn i_border_image_width_one() -> Value {
    Value::BorderImageWidths([
        BorderImageWidthSide::Number(1.0),
        BorderImageWidthSide::Number(1.0),
        BorderImageWidthSide::Number(1.0),
        BorderImageWidthSide::Number(1.0),
    ])
}
fn i_icon_palette_default() -> Value {
    Value::IconPalette(Rc::from(vec![
        (
            Rc::from("error"),
            ColorValue::Named(Rc::from("error_color")),
        ),
        (
            Rc::from("warning"),
            ColorValue::Named(Rc::from("warning_color")),
        ),
        (
            Rc::from("success"),
            ColorValue::Named(Rc::from("success_color")),
        ),
    ]))
}
fn i_transition_property_all() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::All)]))
}
fn i_time_zero_list() -> Value {
    Value::List(Rc::from(vec![Value::Time(Time::ZERO)]))
}
fn i_timing_ease_list() -> Value {
    Value::List(Rc::from(vec![Value::Timing(TimingFunction::EASE)]))
}
fn i_animation_name_none_list() -> Value {
    Value::List(Rc::from(vec![Value::AnimationName(AnimationName::None)]))
}
fn i_iteration_one_list() -> Value {
    Value::List(Rc::from(vec![Value::IterationCount(
        IterationCount::Count(1.0),
    )]))
}
fn i_normal_keyword_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::Normal)]))
}
fn i_running_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::Running)]))
}
fn i_none_keyword_list() -> Value {
    Value::List(Rc::from(vec![Value::Keyword(Keyword::None)]))
}

// ---- the shorthands' longhand lists -----------------------------------

const FONT_LONGHANDS: &[Prop] = &[
    Prop::FontStyle,
    Prop::FontVariant,
    Prop::FontWeight,
    Prop::FontStretch,
    Prop::FontSize,
    Prop::LineHeight,
    Prop::FontFamily,
];
const TEXT_DECORATION_LONGHANDS: &[Prop] = &[
    Prop::TextDecorationLine,
    Prop::TextDecorationStyle,
    Prop::TextDecorationColor,
];
const MARGIN_LONGHANDS: &[Prop] = &[
    Prop::MarginTop,
    Prop::MarginRight,
    Prop::MarginBottom,
    Prop::MarginLeft,
];
const PADDING_LONGHANDS: &[Prop] = &[
    Prop::PaddingTop,
    Prop::PaddingRight,
    Prop::PaddingBottom,
    Prop::PaddingLeft,
];
const BORDER_WIDTH_LONGHANDS: &[Prop] = &[
    Prop::BorderTopWidth,
    Prop::BorderRightWidth,
    Prop::BorderBottomWidth,
    Prop::BorderLeftWidth,
];
const BORDER_STYLE_LONGHANDS: &[Prop] = &[
    Prop::BorderTopStyle,
    Prop::BorderRightStyle,
    Prop::BorderBottomStyle,
    Prop::BorderLeftStyle,
];
const BORDER_COLOR_LONGHANDS: &[Prop] = &[
    Prop::BorderTopColor,
    Prop::BorderRightColor,
    Prop::BorderBottomColor,
    Prop::BorderLeftColor,
];
const BORDER_TOP_LONGHANDS: &[Prop] = &[
    Prop::BorderTopWidth,
    Prop::BorderTopStyle,
    Prop::BorderTopColor,
];
const BORDER_RIGHT_LONGHANDS: &[Prop] = &[
    Prop::BorderRightWidth,
    Prop::BorderRightStyle,
    Prop::BorderRightColor,
];
const BORDER_BOTTOM_LONGHANDS: &[Prop] = &[
    Prop::BorderBottomWidth,
    Prop::BorderBottomStyle,
    Prop::BorderBottomColor,
];
const BORDER_LEFT_LONGHANDS: &[Prop] = &[
    Prop::BorderLeftWidth,
    Prop::BorderLeftStyle,
    Prop::BorderLeftColor,
];
const BORDER_LONGHANDS: &[Prop] = &[
    Prop::BorderTopWidth,
    Prop::BorderTopStyle,
    Prop::BorderTopColor,
    Prop::BorderRightWidth,
    Prop::BorderRightStyle,
    Prop::BorderRightColor,
    Prop::BorderBottomWidth,
    Prop::BorderBottomStyle,
    Prop::BorderBottomColor,
    Prop::BorderLeftWidth,
    Prop::BorderLeftStyle,
    Prop::BorderLeftColor,
];
const BORDER_RADIUS_LONGHANDS: &[Prop] = &[
    Prop::BorderTopLeftRadius,
    Prop::BorderTopRightRadius,
    Prop::BorderBottomRightRadius,
    Prop::BorderBottomLeftRadius,
];
const BORDER_IMAGE_LONGHANDS: &[Prop] = &[
    Prop::BorderImageSource,
    Prop::BorderImageSlice,
    Prop::BorderImageWidth,
    Prop::BorderImageRepeat,
];
const OUTLINE_LONGHANDS: &[Prop] = &[Prop::OutlineWidth, Prop::OutlineStyle, Prop::OutlineColor];
const BACKGROUND_LONGHANDS: &[Prop] = &[
    Prop::BackgroundColor,
    Prop::BackgroundImage,
    Prop::BackgroundPosition,
    Prop::BackgroundSize,
    Prop::BackgroundRepeat,
    Prop::BackgroundOrigin,
    Prop::BackgroundClip,
];
const TRANSITION_LONGHANDS: &[Prop] = &[
    Prop::TransitionProperty,
    Prop::TransitionDuration,
    Prop::TransitionTimingFunction,
    Prop::TransitionDelay,
];
const ANIMATION_LONGHANDS: &[Prop] = &[
    Prop::AnimationName,
    Prop::AnimationDuration,
    Prop::AnimationTimingFunction,
    Prop::AnimationIterationCount,
    Prop::AnimationDirection,
    Prop::AnimationPlayState,
    Prop::AnimationDelay,
    Prop::AnimationFillMode,
];

/// Indexed by `prop as usize`. Row order == `Prop` discriminant order.
pub static PROPERTIES: [PropertyDef; N_PROPS] = [
    // colours & effects
    lh("color", p_color, i_black, true, Some(interpolate::color)),
    lh(
        "opacity",
        p_opacity,
        i_one,
        false,
        Some(interpolate::number),
    ),
    lh(
        "filter",
        p_filter,
        i_filter_none,
        false,
        Some(interpolate::filter_list),
    ),
    // fonts
    lh("font-family", p_family, i_family_sans, true, None),
    lh(
        "font-size",
        p_font_size,
        i_len_14,
        true,
        Some(interpolate::length),
    ),
    lh("font-style", p_font_style, i_style_normal, true, None),
    lh("font-variant", p_variant_css2, i_variant_empty, true, None),
    lh(
        "font-weight",
        p_font_weight,
        i_weight_400,
        true,
        Some(interpolate::font_weight),
    ),
    lh(
        "font-width",
        p_stretch,
        i_pct_one,
        true,
        Some(interpolate::percentage),
    ),
    lh(
        "font-stretch",
        p_stretch,
        i_pct_one,
        true,
        Some(interpolate::percentage),
    ),
    lh("font-kerning", p_kerning, i_auto_keyword, true, None),
    lh(
        "font-variant-ligatures",
        p_variant_ligatures,
        i_variant_empty,
        true,
        None,
    ),
    lh(
        "font-variant-position",
        p_variant_position,
        i_variant_empty,
        true,
        None,
    ),
    lh(
        "font-variant-caps",
        p_variant_caps,
        i_variant_empty,
        true,
        None,
    ),
    lh(
        "font-variant-numeric",
        p_variant_numeric,
        i_variant_empty,
        true,
        None,
    ),
    lh(
        "font-variant-alternates",
        p_variant_alternates,
        i_variant_empty,
        true,
        None,
    ),
    lh(
        "font-variant-east-asian",
        p_variant_east_asian,
        i_variant_empty,
        true,
        None,
    ),
    lh(
        "font-feature-settings",
        p_features,
        i_features_empty,
        true,
        None,
    ),
    lh(
        "font-variation-settings",
        p_variations,
        i_variations_empty,
        true,
        None,
    ),
    lh("-gtk-dpi", p_number, i_dpi, true, None),
    // text
    lh(
        "caret-color",
        p_caret_color,
        i_current_color,
        true,
        Some(interpolate::color),
    ),
    // NOT AVAILABLE from GTK docs; chosen fallback: currentColor, by analogy
    // with `caret-color`.
    lh(
        "-gtk-secondary-caret-color",
        p_color,
        i_current_color,
        true,
        Some(interpolate::color),
    ),
    lh(
        "letter-spacing",
        p_letter_spacing,
        i_zero_length,
        true,
        Some(interpolate::length),
    ),
    lh(
        "text-transform",
        p_text_transform,
        i_none_keyword,
        true,
        None,
    ),
    lh(
        "line-height",
        p_line_height,
        i_line_height_normal,
        true,
        Some(interpolate::line_height),
    ),
    lh(
        "text-decoration-line",
        p_decoration_lines,
        i_decoration_none,
        false,
        None,
    ),
    lh(
        "text-decoration-color",
        p_color,
        i_current_color,
        false,
        Some(interpolate::color),
    ),
    lh(
        "text-decoration-style",
        p_decoration_style,
        i_solid_keyword,
        false,
        None,
    ),
    lh(
        "text-shadow",
        p_text_shadow_list,
        i_empty_list,
        true,
        Some(interpolate::shadow_list),
    ),
    // icons: parsed and stored; drawn in M4
    lh(
        "-gtk-icon-source",
        p_icon_source,
        i_builtin_keyword,
        false,
        None,
    ),
    // NOT AVAILABLE from GTK docs; chosen fallback: 16px, GTK's default icon size.
    lh(
        "-gtk-icon-size",
        p_length,
        i_len_16,
        true,
        Some(interpolate::length),
    ),
    // NOT AVAILABLE from GTK docs; chosen fallback: `requested`.
    lh(
        "-gtk-icon-style",
        p_icon_style,
        i_requested_keyword,
        true,
        None,
    ),
    lh(
        "-gtk-icon-transform",
        p_transform,
        i_transform_none,
        true,
        Some(interpolate::transform),
    ),
    lh(
        "-gtk-icon-palette",
        p_icon_palette,
        i_icon_palette_default,
        true,
        None,
    ),
    lh(
        "-gtk-icon-shadow",
        p_text_shadow_list,
        i_empty_list,
        true,
        Some(interpolate::shadow_list),
    ),
    lh(
        "-gtk-icon-filter",
        p_filter,
        i_filter_none,
        true,
        Some(interpolate::filter_list),
    ),
    // NOT AVAILABLE from GTK docs; chosen fallback: 400, "like font weight".
    lh(
        "-gtk-icon-weight",
        p_font_weight,
        i_weight_400,
        true,
        Some(interpolate::font_weight),
    ),
    // transform
    lh(
        "transform",
        p_transform,
        i_transform_none,
        false,
        Some(interpolate::transform),
    ),
    lh(
        "transform-origin",
        p_position,
        i_position_center,
        false,
        None,
    ),
    // box model
    lh(
        "min-width",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "min-height",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "margin-top",
        p_length_auto,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "margin-right",
        p_length_auto,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "margin-bottom",
        p_length_auto,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "margin-left",
        p_length_auto,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "padding-top",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "padding-right",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "padding-bottom",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    lh(
        "padding-left",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    // borders
    lh(
        "border-top-width",
        p_line_width,
        i_len_3,
        false,
        Some(interpolate::length),
    ),
    lh(
        "border-right-width",
        p_line_width,
        i_len_3,
        false,
        Some(interpolate::length),
    ),
    lh(
        "border-bottom-width",
        p_line_width,
        i_len_3,
        false,
        Some(interpolate::length),
    ),
    lh(
        "border-left-width",
        p_line_width,
        i_len_3,
        false,
        Some(interpolate::length),
    ),
    lh(
        "border-top-style",
        p_line_style,
        i_none_keyword,
        false,
        None,
    ),
    lh(
        "border-right-style",
        p_line_style,
        i_none_keyword,
        false,
        None,
    ),
    lh(
        "border-bottom-style",
        p_line_style,
        i_none_keyword,
        false,
        None,
    ),
    lh(
        "border-left-style",
        p_line_style,
        i_none_keyword,
        false,
        None,
    ),
    lh(
        "border-top-left-radius",
        p_radius,
        i_radius_zero,
        false,
        Some(interpolate::pair),
    ),
    lh(
        "border-top-right-radius",
        p_radius,
        i_radius_zero,
        false,
        Some(interpolate::pair),
    ),
    lh(
        "border-bottom-right-radius",
        p_radius,
        i_radius_zero,
        false,
        Some(interpolate::pair),
    ),
    lh(
        "border-bottom-left-radius",
        p_radius,
        i_radius_zero,
        false,
        Some(interpolate::pair),
    ),
    lh(
        "border-top-color",
        p_color,
        i_current_color,
        false,
        Some(interpolate::color),
    ),
    lh(
        "border-right-color",
        p_color,
        i_current_color,
        false,
        Some(interpolate::color),
    ),
    lh(
        "border-bottom-color",
        p_color,
        i_current_color,
        false,
        Some(interpolate::color),
    ),
    lh(
        "border-left-color",
        p_color,
        i_current_color,
        false,
        Some(interpolate::color),
    ),
    lh(
        "border-image-source",
        p_image,
        i_image_none,
        false,
        Some(interpolate::image),
    ),
    lh(
        "border-image-repeat",
        p_border_image_repeat,
        i_repeat_stretch,
        false,
        None,
    ),
    lh(
        "border-image-slice",
        p_border_image_slice,
        i_slice_full,
        false,
        None,
    ),
    lh(
        "border-image-width",
        p_border_image_width,
        i_border_image_width_one,
        false,
        None,
    ),
    // outline
    lh("outline-style", p_line_style, i_none_keyword, false, None),
    lh(
        "outline-width",
        p_line_width,
        i_len_3,
        false,
        Some(interpolate::length),
    ),
    lh(
        "outline-color",
        p_color,
        i_current_color,
        false,
        Some(interpolate::color),
    ),
    lh(
        "outline-offset",
        p_length,
        i_zero_length,
        false,
        Some(interpolate::length),
    ),
    // backgrounds
    lh(
        "background-color",
        p_color,
        i_transparent,
        false,
        Some(interpolate::color),
    ),
    lh(
        "background-clip",
        p_box_list,
        i_clip_border_list,
        false,
        None,
    ),
    lh(
        "background-origin",
        p_box_list,
        i_origin_padding_list,
        false,
        None,
    ),
    lh(
        "background-size",
        p_bg_size_list,
        i_bg_size_auto_list,
        false,
        Some(interpolate::list),
    ),
    lh(
        "background-position",
        p_position_list,
        i_position_origin_list,
        false,
        Some(interpolate::list),
    ),
    lh(
        "background-repeat",
        p_bg_repeat_list,
        i_bg_repeat_list,
        false,
        None,
    ),
    lh(
        "background-image",
        p_image_list,
        i_image_none_list,
        false,
        Some(interpolate::list),
    ),
    lh(
        "box-shadow",
        p_shadow_list,
        i_empty_list,
        false,
        Some(interpolate::shadow_list),
    ),
    lh(
        "background-blend-mode",
        p_blend_list,
        i_blend_normal_list,
        false,
        None,
    ),
    // transitions (meta-properties: never themselves animatable)
    lh(
        "transition-property",
        p_transition_property_list,
        i_transition_property_all,
        false,
        None,
    ),
    lh(
        "transition-duration",
        p_time_list,
        i_time_zero_list,
        false,
        None,
    ),
    lh(
        "transition-timing-function",
        p_timing_list,
        i_timing_ease_list,
        false,
        None,
    ),
    lh(
        "transition-delay",
        p_time_list,
        i_time_zero_list,
        false,
        None,
    ),
    // animations
    lh(
        "animation-name",
        p_animation_name_list,
        i_animation_name_none_list,
        false,
        None,
    ),
    lh(
        "animation-duration",
        p_time_list,
        i_time_zero_list,
        false,
        None,
    ),
    lh(
        "animation-timing-function",
        p_timing_list,
        i_timing_ease_list,
        false,
        None,
    ),
    lh(
        "animation-iteration-count",
        p_iteration_list,
        i_iteration_one_list,
        false,
        None,
    ),
    lh(
        "animation-direction",
        p_direction_list,
        i_normal_keyword_list,
        false,
        None,
    ),
    lh(
        "animation-play-state",
        p_play_state_list,
        i_running_list,
        false,
        None,
    ),
    lh(
        "animation-delay",
        p_time_list,
        i_time_zero_list,
        false,
        None,
    ),
    lh(
        "animation-fill-mode",
        p_fill_mode_list,
        i_none_keyword_list,
        false,
        None,
    ),
    // misc
    lh(
        "border-spacing",
        p_border_spacing,
        i_spacing_zero,
        true,
        Some(interpolate::pair),
    ),
    // ---- shorthands ----
    sh("font", sh_fns::expand_font, FONT_LONGHANDS),
    sh(
        "text-decoration",
        sh_fns::expand_text_decoration,
        TEXT_DECORATION_LONGHANDS,
    ),
    sh("margin", sh_fns::expand_margin, MARGIN_LONGHANDS),
    sh("padding", sh_fns::expand_padding, PADDING_LONGHANDS),
    sh(
        "border-width",
        sh_fns::expand_border_width,
        BORDER_WIDTH_LONGHANDS,
    ),
    sh(
        "border-style",
        sh_fns::expand_border_style,
        BORDER_STYLE_LONGHANDS,
    ),
    sh(
        "border-color",
        sh_fns::expand_border_color,
        BORDER_COLOR_LONGHANDS,
    ),
    sh(
        "border-top",
        sh_fns::expand_border_top,
        BORDER_TOP_LONGHANDS,
    ),
    sh(
        "border-right",
        sh_fns::expand_border_right,
        BORDER_RIGHT_LONGHANDS,
    ),
    sh(
        "border-bottom",
        sh_fns::expand_border_bottom,
        BORDER_BOTTOM_LONGHANDS,
    ),
    sh(
        "border-left",
        sh_fns::expand_border_left,
        BORDER_LEFT_LONGHANDS,
    ),
    sh("border", sh_fns::expand_border, BORDER_LONGHANDS),
    sh(
        "border-radius",
        sh_fns::expand_border_radius,
        BORDER_RADIUS_LONGHANDS,
    ),
    sh(
        "border-image",
        sh_fns::expand_border_image,
        BORDER_IMAGE_LONGHANDS,
    ),
    sh("outline", sh_fns::expand_outline, OUTLINE_LONGHANDS),
    sh(
        "background",
        sh_fns::expand_background,
        BACKGROUND_LONGHANDS,
    ),
    sh(
        "transition",
        sh_fns::expand_transition,
        TRANSITION_LONGHANDS,
    ),
    sh("animation", sh_fns::expand_animation, ANIMATION_LONGHANDS),
];

impl Prop {
    /// This property's registry row.
    #[must_use]
    pub fn def(self) -> &'static PropertyDef {
        &PROPERTIES[self as usize]
    }

    /// Whether the property inherits; always `false` for a shorthand.
    #[must_use]
    pub fn is_inherited(self) -> bool {
        match self.def().kind {
            PropertyKind::Longhand { inherited, .. } => inherited,
            PropertyKind::Shorthand { .. } => false,
        }
    }

    /// The property's initial value.
    ///
    /// # Panics
    /// Panics for a shorthand, which has no computed slot of its own.
    #[must_use]
    pub fn initial(self) -> Value {
        match self.def().kind {
            PropertyKind::Longhand { initial, .. } => initial(),
            PropertyKind::Shorthand { .. } => {
                panic!("{} is a shorthand and has no initial value", self.name())
            }
        }
    }

    /// The property's interpolator, if it animates.
    #[must_use]
    pub fn interpolator(self) -> Option<Interpolate> {
        match self.def().kind {
            PropertyKind::Longhand { animatable, .. } => animatable,
            PropertyKind::Shorthand { .. } => None,
        }
    }

    /// Expand a shorthand into `sink`. `Err(())` for a longhand or for a
    /// value the shorthand cannot parse.
    pub fn expand_into(
        self,
        input: &mut Parser<'_, '_>,
        sink: &mut dyn FnMut(Prop, Value),
    ) -> Result<(), ()> {
        match self.def().kind {
            PropertyKind::Shorthand { expand, .. } => expand(input, sink),
            PropertyKind::Longhand { .. } => Err(()),
        }
    }

    /// The longhands this shorthand sets, in reset order; empty for a longhand.
    #[must_use]
    pub fn longhands(self) -> &'static [Prop] {
        match self.def().kind {
            PropertyKind::Shorthand { longhands, .. } => longhands,
            PropertyKind::Longhand { .. } => &[],
        }
    }
}

/// Every animatable longhand -- the expansion of `transition-property: all`.
pub fn animatable_longhands() -> impl Iterator<Item = Prop> {
    longhands().filter(|prop| prop.interpolator().is_some())
}

/// Parse a whole declaration value for `prop`.
///
/// The one entry point the cascade needs for a **longhand**: it runs the
/// row's `ParseFn` against `text` and requires the whole input to be
/// consumed. A shorthand has no single value of its own, so it is `Err(())`
/// here; callers fan it out through [`Prop::expand_into`] instead.
/// `Err(())` == invalid at parse time, so the declaration is dropped.
pub fn parse_declaration_value(prop: Prop, text: &str) -> Result<Value, ()> {
    let PropertyKind::Longhand { parse, .. } = prop.def().kind else {
        return Err(());
    };
    let mut source = ParserInput::new(text);
    let mut parser = Parser::new(&mut source);
    let value = parse(&mut parser)?;
    parser.skip_whitespace();
    if parser.is_exhausted() {
        Ok(value)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::{N_LONGHANDS, N_PROPS, Prop, longhands, lookup};
    use super::{PROPERTIES, animatable_longhands, parse_declaration_value};
    use crate::css::value::Value;
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;

    #[test]
    fn the_discriminants_partition_longhands_from_shorthands() {
        assert_eq!(N_LONGHANDS, 95);
        assert_eq!(N_PROPS, 113);
        assert_eq!(Prop::Color as usize, 0);
        assert_eq!(Prop::BorderSpacing as usize, N_LONGHANDS - 1);
        assert_eq!(Prop::Font as usize, N_LONGHANDS);
        assert_eq!(Prop::Animation as usize, N_PROPS - 1);
        assert!(Prop::BorderSpacing.is_longhand());
        assert!(!Prop::Font.is_longhand());
        assert_eq!(Prop::PaddingLeft.slot(), Prop::PaddingLeft as usize);
        assert_eq!(longhands().count(), N_LONGHANDS);
        assert!(longhands().all(Prop::is_longhand));
    }

    #[test]
    fn every_property_has_a_unique_lowercase_name_that_round_trips() {
        let mut names: Vec<&'static str> =
            (0..N_PROPS).map(|index| Prop::ALL[index].name()).collect();
        for name in &names {
            assert_eq!(*name, name.to_ascii_lowercase(), "{name} is not lowercase");
        }
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate property name");
        for prop in Prop::ALL {
            assert_eq!(lookup(prop.name()), Some(*prop));
        }
    }

    #[test]
    fn lookup_is_ascii_case_insensitive_and_rejects_unknown_names() {
        assert_eq!(lookup("BACKGROUND-color"), Some(Prop::BackgroundColor));
        assert_eq!(lookup("-GTK-DPI"), Some(Prop::GtkDpi));
        assert_eq!(lookup("border-radius"), Some(Prop::BorderRadius));
        assert_eq!(lookup("-gtk-outline-radius"), None);
        assert_eq!(lookup(""), None);
        assert_eq!(lookup("\u{e9}"), None);
    }

    #[test]
    fn the_gtk_only_rows_are_present_under_their_exact_spellings() {
        for name in [
            "-gtk-dpi",
            "-gtk-secondary-caret-color",
            "-gtk-icon-source",
            "-gtk-icon-size",
            "-gtk-icon-style",
            "-gtk-icon-transform",
            "-gtk-icon-palette",
            "-gtk-icon-shadow",
            "-gtk-icon-filter",
            "-gtk-icon-weight",
        ] {
            assert!(
                lookup(name).is_some(),
                "{name} is missing from the registry"
            );
        }
    }

    #[test]
    fn the_table_agrees_with_the_enum_on_every_row() {
        assert_eq!(PROPERTIES.len(), N_PROPS);
        for prop in Prop::ALL {
            assert_eq!(prop.def().name, prop.name(), "row {prop:?} is misplaced");
            assert_eq!(
                prop.is_longhand(),
                matches!(prop.def().kind, super::PropertyKind::Longhand { .. }),
                "{prop:?} kind disagrees with its position"
            );
        }
    }

    #[test]
    fn the_settled_initial_values_are_what_the_contract_pins() {
        assert_eq!(Prop::MinWidth.initial(), Value::Length(Length::zero()));
        assert_eq!(Prop::MinHeight.initial(), Value::Length(Length::zero()));
        assert_eq!(
            Prop::BorderTopWidth.initial(),
            Value::Length(Length::px(3.0))
        );
        assert_eq!(Prop::OutlineWidth.initial(), Value::Length(Length::px(3.0)));
        assert_eq!(Prop::OutlineStyle.initial(), Value::Keyword(Keyword::None));
        assert_eq!(Prop::FontSize.initial(), Value::Length(Length::px(14.0)));
        assert_eq!(Prop::GtkDpi.initial(), Value::Number(96.0));
        assert_eq!(Prop::GtkIconSize.initial(), Value::Length(Length::px(16.0)));
        assert_eq!(
            Prop::GtkIconStyle.initial(),
            Value::Keyword(Keyword::Requested)
        );
        assert_eq!(
            Prop::OutlineColor.initial(),
            Value::Color(crate::css::value::color::ColorValue::CurrentColor)
        );
        assert_eq!(
            Prop::Color.initial(),
            Value::Color(crate::css::value::color::ColorValue::Absolute(
                crate::css::value::color::Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0
                }
            ))
        );
    }

    #[test]
    fn the_inherited_flags_follow_gtk() {
        for prop in [
            Prop::Color,
            Prop::FontFamily,
            Prop::FontSize,
            Prop::LineHeight,
            Prop::LetterSpacing,
            Prop::TextTransform,
            Prop::CaretColor,
            Prop::GtkDpi,
            Prop::TextShadow,
            Prop::GtkIconSize,
            Prop::GtkIconShadow,
            Prop::BorderSpacing,
        ] {
            assert!(prop.is_inherited(), "{prop:?} should inherit");
        }
        for prop in [
            Prop::Opacity,
            Prop::Filter,
            Prop::Transform,
            Prop::MinWidth,
            Prop::PaddingTop,
            Prop::BorderTopWidth,
            Prop::BorderTopColor,
            Prop::BackgroundColor,
            Prop::BoxShadow,
            Prop::OutlineColor,
            Prop::TextDecorationLine,
            Prop::TransitionDuration,
            Prop::AnimationName,
        ] {
            assert!(!prop.is_inherited(), "{prop:?} should not inherit");
        }
        // A shorthand never reports itself as inherited.
        assert!(!Prop::Font.is_inherited());
        assert!(!Prop::Background.is_inherited());
    }

    #[test]
    fn every_animatable_row_has_an_interpolator_and_the_meta_rows_do_not() {
        assert!(Prop::BackgroundColor.interpolator().is_some());
        assert!(Prop::PaddingTop.interpolator().is_some());
        assert!(Prop::BoxShadow.interpolator().is_some());
        assert!(Prop::Transform.interpolator().is_some());
        assert!(Prop::TransitionDuration.interpolator().is_none());
        assert!(Prop::AnimationName.interpolator().is_none());
        assert!(Prop::FontFamily.interpolator().is_none());
        assert!(Prop::Font.interpolator().is_none());
        let animatable: Vec<Prop> = animatable_longhands().collect();
        assert!(animatable.iter().all(|prop| prop.is_longhand()));
        assert!(animatable.contains(&Prop::BackgroundColor));
        assert!(!animatable.contains(&Prop::TransitionDelay));
    }

    #[test]
    fn every_shorthand_lists_the_longhands_its_expander_emits() {
        for prop in Prop::ALL.iter().copied().filter(|prop| !prop.is_longhand()) {
            let listed = prop.longhands();
            assert!(!listed.is_empty(), "{prop:?} lists no longhands");
            assert!(
                listed.iter().all(|inner| inner.is_longhand()),
                "{prop:?} lists a shorthand"
            );
        }
        assert_eq!(Prop::MinWidth.longhands(), &[] as &[Prop]);
        assert_eq!(
            Prop::Margin.longhands(),
            &[
                Prop::MarginTop,
                Prop::MarginRight,
                Prop::MarginBottom,
                Prop::MarginLeft
            ]
        );
        assert_eq!(Prop::Border.longhands().len(), 12);
        assert_eq!(Prop::Animation.longhands().len(), 8);
    }

    #[test]
    fn a_shorthand_emits_every_longhand_it_lists() {
        // The CSS reset rule: an omitted component still lands, at its
        // initial value, or an earlier rule's value survives when it must not.
        for (prop, text) in [
            (Prop::Margin, "4px"),
            (Prop::Padding, "4px 9px"),
            (Prop::Border, "1px solid"),
            (Prop::BorderTop, "1px solid red"),
            (Prop::BorderWidth, "1px"),
            (Prop::BorderStyle, "solid"),
            (Prop::BorderColor, "red"),
            (Prop::BorderRadius, "5px"),
            (Prop::BorderImage, "url(\"b.png\") 30%"),
            (Prop::Outline, "2px solid red"),
            (Prop::Background, "#112233"),
            (Prop::Transition, "background-color 200ms"),
            (Prop::Animation, "spin 1s linear"),
            (Prop::Font, "14px Cantarell"),
            (Prop::TextDecoration, "underline"),
            (Prop::BorderRight, "1px solid red"),
            (Prop::BorderBottom, "1px solid red"),
            (Prop::BorderLeft, "1px solid red"),
        ] {
            let mut source = cssparser::ParserInput::new(text);
            let mut parser = cssparser::Parser::new(&mut source);
            let mut emitted: Vec<Prop> = Vec::new();
            {
                let mut sink = |longhand: Prop, _value: Value| emitted.push(longhand);
                prop.expand_into(&mut parser, &mut sink)
                    .unwrap_or_else(|()| panic!("{prop:?} failed to expand `{text}`"));
            }
            for longhand in prop.longhands() {
                assert!(
                    emitted.contains(longhand),
                    "{prop:?} did not emit {longhand:?} for `{text}`"
                );
            }
        }
    }

    #[test]
    fn every_longhand_accepts_the_wide_keywords() {
        for prop in longhands() {
            for keyword in ["inherit", "initial", "unset"] {
                assert!(
                    parse_declaration_value(prop, keyword).is_ok(),
                    "{prop:?} rejected `{keyword}`"
                );
            }
        }
    }

    #[test]
    fn adwaitas_declared_button_values_all_parse() {
        // The exact declarations the offscreen pixel gate pins.
        //
        // `parse_declaration_value` is the *longhand* entry point (a
        // shorthand has no single value to return), so the four shorthand
        // declarations below go through `expand_into`, which is the entry
        // point the cascade uses for them.
        for (prop, text) in [
            (Prop::BorderRadius, "5px"),
            (Prop::Padding, "4px 9px"),
            (Prop::Border, "1px solid"),
            (Prop::BorderColor, "#cdc7c2"),
        ] {
            let mut source = cssparser::ParserInput::new(text);
            let mut parser = cssparser::Parser::new(&mut source);
            let mut sink = |_: Prop, _: Value| {};
            assert!(
                prop.expand_into(&mut parser, &mut sink).is_ok(),
                "{prop:?} rejected `{text}`"
            );
            // A shorthand has no value of its own.
            assert!(parse_declaration_value(prop, text).is_err());
        }
        assert!(parse_declaration_value(Prop::Color, "#2e3436").is_ok());
        assert!(parse_declaration_value(Prop::MinHeight, "24px").is_ok());
        assert!(parse_declaration_value(Prop::MinWidth, "16px").is_ok());
        assert!(
            parse_declaration_value(
                Prop::BackgroundImage,
                "linear-gradient(to top, #f6f5f4 2px, #fbfafa)"
            )
            .is_ok()
        );
        // A trailing token invalidates the declaration.
        assert!(parse_declaration_value(Prop::MinWidth, "16px 4px").is_err());
    }

    #[test]
    fn no_registry_parser_panics_on_hostile_input() {
        for prop in Prop::ALL {
            for input in crate::css::value::FUZZ_INPUTS {
                let _ = parse_declaration_value(*prop, input);
            }
        }
    }
}
