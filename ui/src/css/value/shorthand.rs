//! Shorthand expansion.
//!
//! Every expander emits a value for **every** longhand its registry row
//! lists: an omitted component is emitted at its initial value, which is
//! CSS's shorthand reset rule and the reason `border: none` clears a
//! colour an earlier rule set.

use std::rc::Rc;

use cssparser::{Parser, ParserState, Token};

use crate::css::registry::{Prop, longhands};

use super::Value;
use super::border::{
    BgSize, BorderImageSlice, BorderImageWidthSide, RepeatStyle, four_sides, parse_line_style,
    parse_line_width,
};
use super::color::{ColorValue, Rgba};
use super::font::{
    FontFamily, FontStyle, FontVariantFlags, FontWeight, LineHeight, parse_family_list,
    parse_font_size, parse_stretch,
};
use super::image::{Image, Position};
use super::keyword::Keyword;
use super::length::Length;
use super::require_exhausted;
use super::text::TextDecorationLines;
use super::timing::{AnimationName, IterationCount, Time, TimingFunction};

type Sink<'a> = &'a mut dyn FnMut(Prop, Value);

/// Collect 1..=4 values with `parse`, then apply the four-sides rule.
///
/// `pub(crate)` because `border-image-width`'s longhand parser needs exactly
/// this loop; it had its own copy of the subtle `< 5` bound and the
/// reset-on-`Err` discipline, so a fix here silently left it behind.
pub(crate) fn box_sides<T: Clone>(
    input: &mut Parser<'_, '_>,
    parse: fn(&mut Parser<'_, '_>) -> Result<T, ()>,
) -> Result<[T; 4], ()> {
    let mut values: Vec<T> = Vec::new();
    while values.len() < 5 {
        let state = input.state();
        match parse(input) {
            Ok(value) => values.push(value),
            Err(()) => {
                input.reset(&state);
                break;
            }
        }
    }
    four_sides(values).ok_or(())
}

/// `margin`.
pub fn expand_margin(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, Length::parse_allowing_auto)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::MarginTop,
        Prop::MarginRight,
        Prop::MarginBottom,
        Prop::MarginLeft,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Length(value));
    }
    Ok(())
}

/// `padding`.
pub fn expand_padding(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, Length::parse)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::PaddingTop,
        Prop::PaddingRight,
        Prop::PaddingBottom,
        Prop::PaddingLeft,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Length(value));
    }
    Ok(())
}

/// `border-width`.
pub fn expand_border_width(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, parse_line_width)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::BorderTopWidth,
        Prop::BorderRightWidth,
        Prop::BorderBottomWidth,
        Prop::BorderLeftWidth,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Length(value));
    }
    Ok(())
}

/// `border-style`.
pub fn expand_border_style(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, parse_line_style)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::BorderTopStyle,
        Prop::BorderRightStyle,
        Prop::BorderBottomStyle,
        Prop::BorderLeftStyle,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Keyword(value));
    }
    Ok(())
}

/// `border-color`.
pub fn expand_border_color(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let sides = box_sides(input, ColorValue::parse)?;
    require_exhausted(input)?;
    for (prop, value) in [
        Prop::BorderTopColor,
        Prop::BorderRightColor,
        Prop::BorderBottomColor,
        Prop::BorderLeftColor,
    ]
    .into_iter()
    .zip(sides)
    {
        sink(prop, Value::Color(value));
    }
    Ok(())
}

/// The shared `<line-width> || <line-style> || <color>` body.
fn border_side_components(input: &mut Parser<'_, '_>) -> Result<(Length, Keyword, ColorValue), ()> {
    let mut width: Option<Length> = None;
    let mut style: Option<Keyword> = None;
    let mut color: Option<ColorValue> = None;
    loop {
        let state = input.state();
        if style.is_none() {
            if let Ok(parsed) = parse_line_style(input) {
                style = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if width.is_none() {
            if let Ok(parsed) = parse_line_width(input) {
                width = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                color = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if width.is_none() && style.is_none() && color.is_none() {
        return Err(());
    }
    require_exhausted(input)?;
    Ok((
        width.unwrap_or_else(|| Length::px(3.0)),
        style.unwrap_or(Keyword::None),
        color.unwrap_or(ColorValue::CurrentColor),
    ))
}

fn emit_border_side(
    sink: Sink<'_>,
    props: (Prop, Prop, Prop),
    components: &(Length, Keyword, ColorValue),
) {
    sink(props.0, Value::Length(components.0.clone()));
    sink(props.1, Value::Keyword(components.1));
    sink(props.2, Value::Color(components.2.clone()));
}

/// `border-top`.
pub fn expand_border_top(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (
            Prop::BorderTopWidth,
            Prop::BorderTopStyle,
            Prop::BorderTopColor,
        ),
        &components,
    );
    Ok(())
}

/// `border-right`.
pub fn expand_border_right(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (
            Prop::BorderRightWidth,
            Prop::BorderRightStyle,
            Prop::BorderRightColor,
        ),
        &components,
    );
    Ok(())
}

/// `border-bottom`.
pub fn expand_border_bottom(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (
            Prop::BorderBottomWidth,
            Prop::BorderBottomStyle,
            Prop::BorderBottomColor,
        ),
        &components,
    );
    Ok(())
}

/// `border-left`.
pub fn expand_border_left(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    emit_border_side(
        sink,
        (
            Prop::BorderLeftWidth,
            Prop::BorderLeftStyle,
            Prop::BorderLeftColor,
        ),
        &components,
    );
    Ok(())
}

/// `border` -- all four sides' width, style and colour.
pub fn expand_border(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let components = border_side_components(input)?;
    for props in [
        (
            Prop::BorderTopWidth,
            Prop::BorderTopStyle,
            Prop::BorderTopColor,
        ),
        (
            Prop::BorderRightWidth,
            Prop::BorderRightStyle,
            Prop::BorderRightColor,
        ),
        (
            Prop::BorderBottomWidth,
            Prop::BorderBottomStyle,
            Prop::BorderBottomColor,
        ),
        (
            Prop::BorderLeftWidth,
            Prop::BorderLeftStyle,
            Prop::BorderLeftColor,
        ),
    ] {
        emit_border_side(sink, props, &components);
    }
    // CSS Backgrounds 3: `border` also resets `border-image-*`. Emitting the
    // registry's own initial values keeps the reset in one place -- there is
    // no second spelling of "no border image" to drift from.
    for prop in [
        Prop::BorderImageSource,
        Prop::BorderImageSlice,
        Prop::BorderImageWidth,
        Prop::BorderImageRepeat,
    ] {
        sink(prop, prop.initial());
    }
    Ok(())
}

/// `border-radius`, including the `/` elliptical form.
pub fn expand_border_radius(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let horizontal = box_sides(input, Length::parse)?;
    let before_slash = input.state();
    let vertical = if input.expect_delim('/').is_ok() {
        box_sides(input, Length::parse)?
    } else {
        // A failed `expect_delim` has already consumed the token it
        // rejected; put it back so the caller still sees it.
        input.reset(&before_slash);
        horizontal.clone()
    };
    require_exhausted(input)?;
    for (index, prop) in [
        Prop::BorderTopLeftRadius,
        Prop::BorderTopRightRadius,
        Prop::BorderBottomRightRadius,
        Prop::BorderBottomLeftRadius,
    ]
    .into_iter()
    .enumerate()
    {
        sink(
            prop,
            Value::Pair(Rc::new((
                Value::Length(horizontal[index].clone()),
                Value::Length(vertical[index].clone()),
            ))),
        );
    }
    Ok(())
}

/// `border-image`.
pub fn expand_border_image(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let mut source: Option<Image> = None;
    let mut slice: Option<BorderImageSlice> = None;
    let mut widths: Option<[BorderImageWidthSide; 4]> = None;
    let mut repeat: Option<RepeatStyle> = None;
    loop {
        let state = input.state();
        if source.is_none() {
            if let Ok(parsed) = Image::parse(input) {
                source = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        if slice.is_none() {
            if let Ok(parsed) = BorderImageSlice::parse(input) {
                slice = Some(parsed);
                let before_slash = input.state();
                if input.expect_delim('/').is_ok() {
                    widths = Some(box_sides(input, BorderImageWidthSide::parse)?);
                } else {
                    input.reset(&before_slash);
                }
                continue;
            }
            input.reset(&state);
        }
        if repeat.is_none() {
            if let Ok(parsed) = RepeatStyle::parse(input) {
                repeat = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if source.is_none() && slice.is_none() && repeat.is_none() {
        return Err(());
    }
    require_exhausted(input)?;
    sink(
        Prop::BorderImageSource,
        Value::Image(source.unwrap_or(Image::None)),
    );
    sink(
        Prop::BorderImageSlice,
        Value::Slice(slice.unwrap_or(BorderImageSlice {
            sides: [super::border::NumberOrPercent::Percent(1.0); 4],
            fill: false,
        })),
    );
    sink(
        Prop::BorderImageWidth,
        Value::BorderImageWidths(widths.unwrap_or([
            BorderImageWidthSide::Number(1.0),
            BorderImageWidthSide::Number(1.0),
            BorderImageWidthSide::Number(1.0),
            BorderImageWidthSide::Number(1.0),
        ])),
    );
    sink(
        Prop::BorderImageRepeat,
        Value::Repeat(repeat.unwrap_or(RepeatStyle {
            x: Keyword::Stretch,
            y: Keyword::Stretch,
        })),
    );
    Ok(())
}

/// `outline` -- style, width and colour, but never `outline-offset`.
pub fn expand_outline(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let (width, style, color) = border_side_components(input)?;
    sink(Prop::OutlineWidth, Value::Length(width));
    sink(Prop::OutlineStyle, Value::Keyword(style));
    sink(Prop::OutlineColor, Value::Color(color));
    Ok(())
}

/// One `background` layer.
struct BackgroundLayerParts {
    color: Option<ColorValue>,
    image: Option<Image>,
    position: Option<Position>,
    size: Option<BgSize>,
    repeat: Option<RepeatStyle>,
    origin: Option<Keyword>,
    clip: Option<Keyword>,
}

impl BackgroundLayerParts {
    /// Whether this layer carries a colour and nothing else at all.
    fn is_colour_only(&self) -> bool {
        self.color.is_some()
            && self.image.is_none()
            && self.position.is_none()
            && self.size.is_none()
            && self.repeat.is_none()
            && self.origin.is_none()
            && self.clip.is_none()
    }
}

fn parse_box_keyword(input: &mut Parser<'_, '_>) -> Result<Keyword, ()> {
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    for keyword in [
        Keyword::BorderBox,
        Keyword::PaddingBox,
        Keyword::ContentBox,
        Keyword::TextBox,
    ] {
        if name.eq_ignore_ascii_case(keyword.as_str()) {
            return Ok(keyword);
        }
    }
    Err(())
}

fn parse_background_layer(input: &mut Parser<'_, '_>) -> Result<BackgroundLayerParts, ()> {
    let mut parts = BackgroundLayerParts {
        color: None,
        image: None,
        position: None,
        size: None,
        repeat: None,
        origin: None,
        clip: None,
    };
    let mut seen = false;
    loop {
        let state = input.state();
        if parts.image.is_none() {
            if let Ok(parsed) = Image::parse(input) {
                parts.image = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if parts.repeat.is_none() {
            if let Ok(parsed) = RepeatStyle::parse_background(input) {
                parts.repeat = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if parts.origin.is_none() || parts.clip.is_none() {
            if let Ok(parsed) = parse_box_keyword(input) {
                if parts.origin.is_none() {
                    parts.origin = Some(parsed);
                } else {
                    parts.clip = Some(parsed);
                }
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if parts.position.is_none() {
            if let Ok(parsed) = Position::parse(input) {
                parts.position = Some(parsed);
                seen = true;
                let before_slash = input.state();
                if input.expect_delim('/').is_ok() {
                    parts.size = Some(BgSize::parse(input)?);
                } else {
                    input.reset(&before_slash);
                }
                continue;
            }
            input.reset(&state);
        }
        if parts.color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                parts.color = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if seen { Ok(parts) } else { Err(()) }
}

/// `background`.
///
/// GTK is lax about which layer carries the colour (Adwaita:1359 puts it
/// first), so a colour anywhere is accepted and the *last* one wins. A
/// layer that carries only a colour contributes no image layer, which is
/// what keeps "the first layer is the one painted on top" true.
pub fn expand_background(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let layers = input
        .parse_comma_separated(|argument| match parse_background_layer(argument) {
            Ok(layer) => Ok(layer),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if layers.is_empty() {
        return Err(());
    }
    let mut color: Option<ColorValue> = None;
    let mut images: Vec<Value> = Vec::new();
    let mut positions: Vec<Value> = Vec::new();
    let mut sizes: Vec<Value> = Vec::new();
    let mut repeats: Vec<Value> = Vec::new();
    let mut origins: Vec<Value> = Vec::new();
    let mut clips: Vec<Value> = Vec::new();
    for layer in &layers {
        if let Some(parsed) = &layer.color {
            color = Some(parsed.clone());
        }
        // A layer that is *nothing but* a colour still contributes no image
        // layer -- that is the GTK divergence documented above, and what
        // keeps "the first image is the one painted on top" true for
        // Adwaita:1359's leading colour.
        //
        // A layer that carries a colour *and* something else is a different
        // matter: its position, size, repeat, origin and clip apply to that
        // colour, so it becomes an explicit `none` image layer rather than
        // being dropped with all five components. Without this,
        // `background: red content-box` painted red over the padding and
        // border, and `background: #fff no-repeat` came out repeating.
        if layer.is_colour_only() {
            continue;
        }
        images.push(Value::Image(layer.image.clone().unwrap_or(Image::None)));
        positions.push(Value::Position(layer.position.clone().unwrap_or(
            Position {
                x: Length::Percent(0.0),
                y: Length::Percent(0.0),
                z: None,
            },
        )));
        sizes.push(Value::BgSize(layer.size.clone().unwrap_or(BgSize::Auto)));
        repeats.push(Value::Repeat(layer.repeat.unwrap_or(RepeatStyle {
            x: Keyword::Repeat,
            y: Keyword::Repeat,
        })));
        // CSS Backgrounds 3: one `<box>` sets *both* longhands; only when a
        // second one is given do origin and clip differ.
        origins.push(Value::Keyword(layer.origin.unwrap_or(Keyword::PaddingBox)));
        clips.push(Value::Keyword(
            layer.clip.or(layer.origin).unwrap_or(Keyword::BorderBox),
        ));
    }
    if images.is_empty() {
        images.push(Value::Image(Image::None));
        positions.push(Value::Position(Position {
            x: Length::Percent(0.0),
            y: Length::Percent(0.0),
            z: None,
        }));
        sizes.push(Value::BgSize(BgSize::Auto));
        repeats.push(Value::Repeat(RepeatStyle {
            x: Keyword::Repeat,
            y: Keyword::Repeat,
        }));
        origins.push(Value::Keyword(Keyword::PaddingBox));
        clips.push(Value::Keyword(Keyword::BorderBox));
    }
    sink(
        Prop::BackgroundColor,
        Value::Color(color.unwrap_or(ColorValue::Absolute(Rgba::TRANSPARENT))),
    );
    sink(Prop::BackgroundImage, Value::List(images.into()));
    sink(Prop::BackgroundPosition, Value::List(positions.into()));
    sink(Prop::BackgroundSize, Value::List(sizes.into()));
    sink(Prop::BackgroundRepeat, Value::List(repeats.into()));
    sink(Prop::BackgroundOrigin, Value::List(origins.into()));
    sink(Prop::BackgroundClip, Value::List(clips.into()));
    Ok(())
}

/// `text-decoration`.
pub fn expand_text_decoration(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    const STYLES: &[Keyword] = &[
        Keyword::Solid,
        Keyword::Double,
        Keyword::Dotted,
        Keyword::Dashed,
        Keyword::Wavy,
    ];
    // `<line>` keywords are accumulated one at a time here rather than by
    // calling `text::parse_decoration_lines`: that one is a whole-value
    // parser and rejects the `wavy`/`red` that legitimately follow inside
    // the shorthand.
    const LINES: &[(Keyword, TextDecorationLines)] = &[
        (Keyword::Underline, TextDecorationLines::UNDERLINE),
        (Keyword::Overline, TextDecorationLines::OVERLINE),
        (Keyword::LineThrough, TextDecorationLines::LINE_THROUGH),
        (Keyword::Blink, TextDecorationLines::BLINK),
    ];
    let mut lines: Option<TextDecorationLines> = None;
    let mut style = None;
    let mut color = None;
    loop {
        let state = input.state();
        if style.is_none() {
            if let Ok(name) = input.expect_ident() {
                let name = name.as_ref().to_string();
                if let Some(found) = STYLES
                    .iter()
                    .copied()
                    .find(|keyword| name.eq_ignore_ascii_case(keyword.as_str()))
                {
                    style = Some(found);
                    continue;
                }
            }
            input.reset(&state);
        }
        if let Ok(name) = input.expect_ident() {
            let name = name.as_ref().to_string();
            let seen = lines.unwrap_or_default();
            // `none` is exclusive: it is only valid on its own, and
            // it is the only way `lines` becomes `Some(empty)`.
            if name.eq_ignore_ascii_case("none") && lines.is_none() {
                lines = Some(TextDecorationLines::empty());
                continue;
            }
            // A repeated line keyword is invalid, as is any line
            // keyword after `none`: leave it unconsumed so
            // `require_exhausted` rejects the declaration.
            if !(lines.is_some() && seen.is_empty())
                && let Some(flag) = LINES
                    .iter()
                    .find(|(keyword, _)| name.eq_ignore_ascii_case(keyword.as_str()))
                    .map(|(_, flag)| *flag)
                && !seen.contains(flag)
            {
                lines = Some(seen | flag);
                continue;
            }
        }
        input.reset(&state);
        if color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                color = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if lines.is_none() && style.is_none() && color.is_none() {
        return Err(());
    }
    require_exhausted(input)?;
    sink(
        Prop::TextDecorationLine,
        Value::TextDecorationLines(lines.unwrap_or_default()),
    );
    sink(
        Prop::TextDecorationStyle,
        Value::Keyword(style.unwrap_or(Keyword::Solid)),
    );
    sink(
        Prop::TextDecorationColor,
        Value::Color(color.unwrap_or(ColorValue::CurrentColor)),
    );
    Ok(())
}

/// The `<font-size> [/ <line-height>]? <font-family>` tail of a `font`
/// shorthand.
struct FontTail {
    size: Length,
    line_height: Option<LineHeight>,
    families: Rc<[FontFamily]>,
}

/// The `<font-size> [/ <line-height>]? <font-family>` tail of a `font`
/// shorthand, including the requirement that nothing follows it.
///
/// Split out so `expand_font` can try it twice: once with a leading bare
/// `<percentage>` taken as `font-stretch`, and again with it given back to
/// `font-size` (CSS Fonts 3 §3.7).
fn parse_font_tail(input: &mut Parser<'_, '_>) -> Result<FontTail, ()> {
    let size = parse_font_size(input)?;
    let before_slash = input.state();
    let line_height = if input.expect_delim('/').is_ok() {
        Some(LineHeight::parse(input)?)
    } else {
        input.reset(&before_slash);
        None
    };
    let families = parse_family_list(input)?;
    require_exhausted(input)?;
    Ok(FontTail {
        size,
        line_height,
        families,
    })
}

/// `font` -- GTK's form also carries `/ <line-height>`.
pub fn expand_font(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let mut style = None;
    let mut weight = None;
    let mut variant = None;
    let mut stretch = None;
    // Where an ambiguous bare `<percentage>` taken as `font-stretch` began,
    // so it can be handed back to `font-size` if the tail does not parse.
    let mut ambiguous_percentage: Option<ParserState> = None;
    // Each slot is filled at most once, so an explicit initial value --
    // `normal`, `400`, `100%` -- is *consumed* rather than rejected: it fills
    // the first slot that accepts it and the next `normal` moves on to the
    // next slot. Refusing it left the token unconsumed, `parse_font_size` then
    // saw the ident, and the whole `font: normal 12px sans-serif` was dropped.
    loop {
        let state = input.state();
        if style.is_none()
            && let Ok(parsed) = FontStyle::parse(input)
        {
            style = Some(parsed);
            continue;
        }
        input.reset(&state);
        if weight.is_none()
            && let Ok(parsed) = FontWeight::parse(input)
        {
            weight = Some(parsed);
            continue;
        }
        input.reset(&state);
        if variant.is_none()
            && let Ok(parsed) =
                super::font::parse_variant_flags(input, FontVariantFlags::SMALL_CAPS)
        {
            variant = Some(parsed);
            continue;
        }
        input.reset(&state);
        // A stretch *keyword* is never a font-size, so only a percentage is
        // ambiguous; note which this was before `parse_stretch` consumes it.
        let percentage = matches!(input.next(), Ok(Token::Percentage { .. }));
        input.reset(&state);
        if stretch.is_none()
            && let Ok(parsed) = parse_stretch(input)
        {
            // CSS Fonts 3 §3.7: a bare `<percentage>` here is ambiguous --
            // `font-stretch` and `font-size` both take one. It is the stretch
            // only if a `<font-size>` still follows, so remember where it
            // started and be ready to give it back to the size below.
            // `font: 100% serif` was rejected outright: the stretch slot ate
            // the `100%` and `parse_font_size` then found `serif`.
            if percentage {
                ambiguous_percentage = Some(state.clone());
            }
            stretch = Some(parsed);
            continue;
        }
        input.reset(&state);
        break;
    }
    // Every prefix slot precedes the size in the grammar, so if the tail does
    // not parse, the only thing that can be reinterpreted is that percentage,
    // and nothing after it needs re-reading as a prefix keyword.
    let tail = match parse_font_tail(input) {
        Ok(tail) => tail,
        Err(()) => {
            let state = ambiguous_percentage.ok_or(())?;
            input.reset(&state);
            stretch = None;
            parse_font_tail(input)?
        }
    };
    let FontTail {
        size,
        line_height,
        families,
    } = tail;
    sink(
        Prop::FontStyle,
        Value::FontStyle(style.unwrap_or(FontStyle::Normal)),
    );
    sink(
        Prop::FontVariant,
        Value::FontVariant(variant.unwrap_or_default()),
    );
    sink(
        Prop::FontWeight,
        Value::FontWeight(weight.unwrap_or(FontWeight::Absolute(400.0))),
    );
    // `font-width` is `font-stretch`'s CSS Fonts 4 name and the one
    // `text.rs` reads first, so the shorthand has to reset both or a stale
    // `font-width` survives a later `font:` and picks a condensed face.
    let stretch = Value::Percentage(stretch.unwrap_or(100.0) / 100.0);
    sink(Prop::FontStretch, stretch.clone());
    sink(Prop::FontWidth, stretch);
    sink(Prop::FontSize, Value::Length(size));
    sink(
        Prop::LineHeight,
        Value::LineHeight(line_height.unwrap_or(LineHeight::Normal)),
    );
    sink(Prop::FontFamily, Value::FontFamilies(families));
    Ok(())
}

/// One `transition` layer: `[none | <property>] || <time> || <easing> || <time>`.
fn parse_transition_layer(
    input: &mut Parser<'_, '_>,
) -> Result<(Value, Time, TimingFunction, Time), ()> {
    let mut property: Option<Value> = None;
    let mut times: Vec<Time> = Vec::new();
    let mut timing: Option<TimingFunction> = None;
    let mut seen = false;
    loop {
        let state = input.state();
        if times.len() < 2 {
            if let Ok(parsed) = Time::parse(input) {
                times.push(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if timing.is_none() {
            if let Ok(parsed) = TimingFunction::parse(input) {
                timing = Some(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if property.is_none() {
            if let Ok(name) = input.expect_ident() {
                let name = name.as_ref().to_string();
                property = Some(if name.eq_ignore_ascii_case("none") {
                    Value::Keyword(Keyword::None)
                } else if name.eq_ignore_ascii_case("all") {
                    Value::Keyword(Keyword::All)
                } else {
                    Value::AnimationName(AnimationName::Named(Rc::from(name.as_str())))
                });
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if !seen {
        return Err(());
    }
    Ok((
        property.unwrap_or(Value::Keyword(Keyword::All)),
        times.first().copied().unwrap_or(Time::ZERO),
        timing.unwrap_or(TimingFunction::EASE),
        times.get(1).copied().unwrap_or(Time::ZERO),
    ))
}

/// `transition`.
pub fn expand_transition(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let layers = input
        .parse_comma_separated(|argument| match parse_transition_layer(argument) {
            Ok(layer) => Ok(layer),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if layers.is_empty() {
        return Err(());
    }
    let mut properties = Vec::new();
    let mut durations = Vec::new();
    let mut timings = Vec::new();
    let mut delays = Vec::new();
    for (property, duration, timing, delay) in layers {
        properties.push(property);
        durations.push(Value::Time(duration));
        timings.push(Value::Timing(timing));
        delays.push(Value::Time(delay));
    }
    sink(Prop::TransitionProperty, Value::List(properties.into()));
    sink(Prop::TransitionDuration, Value::List(durations.into()));
    sink(Prop::TransitionTimingFunction, Value::List(timings.into()));
    sink(Prop::TransitionDelay, Value::List(delays.into()));
    Ok(())
}

struct AnimationLayer {
    name: AnimationName,
    duration: Time,
    delay: Time,
    timing: TimingFunction,
    iterations: IterationCount,
    direction: Keyword,
    fill: Keyword,
    play_state: Keyword,
}

fn parse_animation_layer(input: &mut Parser<'_, '_>) -> Result<AnimationLayer, ()> {
    const DIRECTIONS: &[Keyword] = &[
        Keyword::Normal,
        Keyword::Reverse,
        Keyword::Alternate,
        Keyword::AlternateReverse,
    ];
    const FILLS: &[Keyword] = &[
        Keyword::None,
        Keyword::Forwards,
        Keyword::Backwards,
        Keyword::Both,
    ];
    const PLAY_STATES: &[Keyword] = &[Keyword::Running, Keyword::Paused];
    let mut layer = AnimationLayer {
        name: AnimationName::None,
        duration: Time::ZERO,
        delay: Time::ZERO,
        timing: TimingFunction::EASE,
        iterations: IterationCount::Count(1.0),
        direction: Keyword::Normal,
        fill: Keyword::None,
        play_state: Keyword::Running,
    };
    let mut times: Vec<Time> = Vec::new();
    let (mut has_timing, mut has_iterations) = (false, false);
    let (mut has_direction, mut has_fill, mut has_play, mut has_name) =
        (false, false, false, false);
    let mut seen = false;
    loop {
        let state = input.state();
        if times.len() < 2 {
            if let Ok(parsed) = Time::parse(input) {
                times.push(parsed);
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if !has_timing {
            if let Ok(parsed) = TimingFunction::parse(input) {
                layer.timing = parsed;
                has_timing = true;
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        if !has_iterations {
            if let Ok(parsed) = IterationCount::parse(input) {
                layer.iterations = parsed;
                has_iterations = true;
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        let keyword = match input.next() {
            Ok(Token::Ident(name)) => Keyword::from_str_ascii_ci(name.as_ref()),
            _ => None,
        };
        if let Some(keyword) = keyword {
            if !has_direction && DIRECTIONS.contains(&keyword) {
                layer.direction = keyword;
                has_direction = true;
                seen = true;
                continue;
            }
            if !has_fill && FILLS.contains(&keyword) && keyword != Keyword::None {
                layer.fill = keyword;
                has_fill = true;
                seen = true;
                continue;
            }
            if !has_play && PLAY_STATES.contains(&keyword) {
                layer.play_state = keyword;
                has_play = true;
                seen = true;
                continue;
            }
        }
        input.reset(&state);
        if !has_name {
            if let Ok(parsed) = AnimationName::parse(input) {
                layer.name = parsed;
                has_name = true;
                seen = true;
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if !seen {
        return Err(());
    }
    layer.duration = times.first().copied().unwrap_or(Time::ZERO);
    layer.delay = times.get(1).copied().unwrap_or(Time::ZERO);
    Ok(layer)
}

/// `animation`.
pub fn expand_animation(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let layers = input
        .parse_comma_separated(|argument| match parse_animation_layer(argument) {
            Ok(layer) => Ok(layer),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if layers.is_empty() {
        return Err(());
    }
    let mut names = Vec::new();
    let mut durations = Vec::new();
    let mut timings = Vec::new();
    let mut iterations = Vec::new();
    let mut directions = Vec::new();
    let mut play_states = Vec::new();
    let mut delays = Vec::new();
    let mut fills = Vec::new();
    for layer in layers {
        names.push(Value::AnimationName(layer.name));
        durations.push(Value::Time(layer.duration));
        timings.push(Value::Timing(layer.timing));
        iterations.push(Value::IterationCount(layer.iterations));
        directions.push(Value::Keyword(layer.direction));
        play_states.push(Value::Keyword(layer.play_state));
        delays.push(Value::Time(layer.delay));
        fills.push(Value::Keyword(layer.fill));
    }
    sink(Prop::AnimationName, Value::List(names.into()));
    sink(Prop::AnimationDuration, Value::List(durations.into()));
    sink(Prop::AnimationTimingFunction, Value::List(timings.into()));
    sink(
        Prop::AnimationIterationCount,
        Value::List(iterations.into()),
    );
    sink(Prop::AnimationDirection, Value::List(directions.into()));
    sink(Prop::AnimationPlayState, Value::List(play_states.into()));
    sink(Prop::AnimationDelay, Value::List(delays.into()));
    sink(Prop::AnimationFillMode, Value::List(fills.into()));
    Ok(())
}

/// `all`. The only value CSS gives this shorthand is a wide keyword
/// (`inherit` / `initial` / `unset`); it resets every longhand to it.
///
/// This body is unreachable by design, and cannot be deleted:
/// `cascade.rs`'s `parse_declaration` intercepts a wide keyword on *any*
/// shorthand before its `ExpandFn` is consulted (via `Prop::longhands()`),
/// and a wide keyword is the only thing `all` accepts -- so the live cascade
/// never calls this. The registry still needs an `ExpandFn` for the `all`
/// row to exist at all, and that row is what makes `all: unset` (which
/// Adwaita uses) a recognised declaration rather than a dropped one. The
/// coverage instrument in `tests/adwaita_coverage.rs` calls it directly, so
/// the expansion it describes is still exercised.
pub fn expand_all(input: &mut Parser<'_, '_>, sink: Sink<'_>) -> Result<(), ()> {
    let wide = Value::parse_wide(input)?;
    for prop in longhands() {
        sink(prop, wide.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::registry::{ExpandFn, Prop};
    use crate::css::value::Value;
    use crate::css::value::color::ColorValue;
    use crate::css::value::image::Image;
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;

    fn expanded(expand: ExpandFn, text: &str) -> Result<Vec<(Prop, Value)>, ()> {
        let mut source = cssparser::ParserInput::new(text);
        let mut parser = cssparser::Parser::new(&mut source);
        let mut out: Vec<(Prop, Value)> = Vec::new();
        {
            let mut sink = |prop: Prop, value: Value| out.push((prop, value));
            expand(&mut parser, &mut sink)?;
        }
        parser.skip_whitespace();
        if parser.is_exhausted() {
            Ok(out)
        } else {
            Err(())
        }
    }

    fn get(pairs: &[(Prop, Value)], prop: Prop) -> Option<&Value> {
        pairs
            .iter()
            .find(|(key, _)| *key == prop)
            .map(|(_, value)| value)
    }

    fn px(value: f32) -> Value {
        Value::Length(Length::px(value))
    }

    #[test]
    fn padding_follows_the_one_to_four_value_rule() {
        let one = expanded(expand_padding, "4px").expect("expands");
        assert_eq!(get(&one, Prop::PaddingLeft), Some(&px(4.0)));
        let two = expanded(expand_padding, "4px 9px").expect("expands");
        assert_eq!(get(&two, Prop::PaddingTop), Some(&px(4.0)));
        assert_eq!(get(&two, Prop::PaddingRight), Some(&px(9.0)));
        assert_eq!(get(&two, Prop::PaddingBottom), Some(&px(4.0)));
        assert_eq!(get(&two, Prop::PaddingLeft), Some(&px(9.0)));
        let three = expanded(expand_padding, "1px 2px 3px").expect("expands");
        assert_eq!(get(&three, Prop::PaddingBottom), Some(&px(3.0)));
        assert_eq!(get(&three, Prop::PaddingLeft), Some(&px(2.0)));
    }

    #[test]
    fn border_splits_function_valued_colours_intact() {
        let pairs = expanded(expand_border, "1px solid rgb(0 0 0)").expect("expands");
        assert_eq!(get(&pairs, Prop::BorderTopWidth), Some(&px(1.0)));
        assert_eq!(
            get(&pairs, Prop::BorderTopStyle),
            Some(&Value::Keyword(Keyword::Solid))
        );
        assert!(matches!(
            get(&pairs, Prop::BorderLeftColor),
            Some(Value::Color(ColorValue::Absolute(_)))
        ));
    }

    #[test]
    fn border_resets_the_components_it_omits() {
        // The reset values are now the registry's initial values: `medium`
        // is 3px and the omitted colour is currentColor.
        let pairs = expanded(expand_border, "none").expect("expands");
        assert_eq!(
            get(&pairs, Prop::BorderTopStyle),
            Some(&Value::Keyword(Keyword::None))
        );
        assert_eq!(get(&pairs, Prop::BorderTopWidth), Some(&px(3.0)));
        assert_eq!(
            get(&pairs, Prop::BorderTopColor),
            Some(&Value::Color(ColorValue::CurrentColor))
        );
        let adwaita = expanded(expand_border, "1px solid").expect("expands");
        assert_eq!(get(&adwaita, Prop::BorderTopWidth), Some(&px(1.0)));
        assert_eq!(
            get(&adwaita, Prop::BorderTopColor),
            Some(&Value::Color(ColorValue::CurrentColor))
        );
    }

    #[test]
    fn a_single_side_border_touches_only_that_side() {
        let pairs = expanded(expand_border_bottom, "2px dashed red").expect("expands");
        assert_eq!(get(&pairs, Prop::BorderBottomWidth), Some(&px(2.0)));
        assert_eq!(get(&pairs, Prop::BorderTopWidth), None);
    }

    #[test]
    fn background_separates_colour_from_image() {
        let flat = expanded(expand_background, "#112233").expect("expands");
        assert!(matches!(
            get(&flat, Prop::BackgroundColor),
            Some(Value::Color(_))
        ));
        assert_eq!(
            get(&flat, Prop::BackgroundImage),
            Some(&Value::List(vec![Value::Image(Image::None)].into()))
        );

        let image = expanded(expand_background, "image(#e8e6e3)").expect("expands");
        assert!(matches!(
            get(&image, Prop::BackgroundImage),
            Some(Value::List(items)) if matches!(items[0], Value::Image(Image::Solid(_)))
        ));
        assert_eq!(
            get(&image, Prop::BackgroundColor),
            Some(&Value::Color(ColorValue::Absolute(
                crate::css::value::color::Rgba::TRANSPARENT
            )))
        );

        let both = expanded(
            expand_background,
            "#dfdcd8 linear-gradient(to top, #dad6d2, #e1dedb)",
        )
        .expect("expands");
        assert!(matches!(
            get(&both, Prop::BackgroundColor),
            Some(Value::Color(_))
        ));
        assert!(matches!(
            get(&both, Prop::BackgroundImage),
            Some(Value::List(items)) if matches!(items[0], Value::Image(Image::Gradient(_)))
        ));

        let none = expanded(expand_background, "none").expect("expands");
        assert_eq!(
            get(&none, Prop::BackgroundImage),
            Some(&Value::List(vec![Value::Image(Image::None)].into()))
        );

        // `no-repeat` is a keyword, not a named colour.
        let keyworded = expanded(expand_background, "image(#f6f5f4) no-repeat").expect("expands");
        assert_eq!(
            get(&keyworded, Prop::BackgroundColor),
            Some(&Value::Color(ColorValue::Absolute(
                crate::css::value::color::Rgba::TRANSPARENT
            )))
        );
    }

    #[test]
    fn a_longhand_has_no_expansion() {
        // M1's "a longhand passes straight through" rule: in M2 a longhand
        // simply is not a shorthand.
        assert!(Prop::MinHeight.is_longhand());
        assert!(!Prop::BorderRadius.is_longhand());
    }

    #[test]
    fn all_resets_every_longhand_to_the_wide_keyword() {
        // `all: unset` is GTK-real (Adwaita's
        // `progressbar > trough.empty > progress { all: unset; }`) and its
        // longhand set is every longhand the registry has.
        // Mutation check: swap `longhands()` for a partial list (say,
        // `FONT_LONGHANDS`) and the count assertion below fails.
        let out = expanded(expand_all, "unset").expect("`all: unset` expands");
        assert_eq!(out.len(), crate::css::registry::longhands().count());
        assert!(
            out.iter()
                .all(|(_, value)| *value == Value::Wide(crate::css::value::Wide::Unset))
        );
        // Only a wide keyword is valid: anything else is a parse error, not
        // a silently-dropped declaration.
        assert!(expanded(expand_all, "1px").is_err());
    }

    #[test]
    fn an_unusable_box_shorthand_is_invalid() {
        // M1 kept the unexpandable value under its own name so the
        // runner-up rule could see it lose. Decision 6 retires that rule:
        // a shorthand that cannot expand is a parse error and the whole
        // declaration is dropped.
        assert!(expanded(expand_padding, "1px 2px 3px 4px 5px").is_err());
    }

    #[test]
    fn line_width_keywords_are_widths_not_colours() {
        // Review round 1: `thin`/`medium`/`thick` are bare identifiers, so
        // the colour test claimed them first.
        let thin = expanded(expand_border, "thin solid").expect("expands");
        assert_eq!(get(&thin, Prop::BorderTopWidth), Some(&px(1.0)));
        assert_eq!(
            get(&thin, Prop::BorderTopColor),
            Some(&Value::Color(ColorValue::CurrentColor))
        );
        let thick = expanded(expand_border, "thick dotted red").expect("expands");
        assert_eq!(get(&thick, Prop::BorderTopWidth), Some(&px(5.0)));
        assert_eq!(
            get(&thick, Prop::BorderTopStyle),
            Some(&Value::Keyword(Keyword::Dotted))
        );
        assert!(matches!(
            get(&thick, Prop::BorderTopColor),
            Some(Value::Color(_))
        ));
        let medium = expanded(expand_border, "medium solid").expect("expands");
        assert_eq!(get(&medium, Prop::BorderTopWidth), Some(&px(3.0)));
    }

    #[test]
    fn a_colour_in_a_non_final_background_layer_is_still_found() {
        // Review round 1: Adwaita:1359 puts the colour in the *first* layer.
        // GTK accepts it; CSS does not. One colour anywhere is unambiguous.
        // A layer that is only a colour contributes no image layer, so the
        // first *image* is still the one painted on top.
        let junction = expanded(
            expand_background,
            "#cdc7c2, linear-gradient(to bottom, transparent 1px, #cecece 1px), linear-gradient(to left, transparent 1px, #cecece 1px)",
        )
        .expect("expands");
        assert!(matches!(
            get(&junction, Prop::BackgroundColor),
            Some(Value::Color(_))
        ));
        match get(&junction, Prop::BackgroundImage) {
            Some(Value::List(items)) => {
                assert_eq!(items.len(), 2, "colour-only layers contribute no image");
                assert!(matches!(items[0], Value::Image(Image::Gradient(_))));
            }
            other => panic!("expected an image list, got {other:?}"),
        }
        // Two candidate colours are ambiguous: only the final layer's counts.
        let ambiguous = expanded(expand_background, "red, blue").expect("expands");
        assert!(matches!(
            get(&ambiguous, Prop::BackgroundColor),
            Some(Value::Color(_))
        ));
    }

    #[test]
    fn the_remaining_shorthands_expand_their_longhands() {
        let radius = expanded(expand_border_radius, "5px").expect("expands");
        assert_eq!(
            get(&radius, Prop::BorderTopLeftRadius),
            Some(&Value::Pair(std::rc::Rc::new((px(5.0), px(5.0)))))
        );
        let elliptical = expanded(expand_border_radius, "10px / 20px").expect("expands");
        assert_eq!(
            get(&elliptical, Prop::BorderTopLeftRadius),
            Some(&Value::Pair(std::rc::Rc::new((px(10.0), px(20.0)))))
        );
        let transition =
            expanded(expand_transition, "background-color 200ms ease-in").expect("expands");
        assert!(get(&transition, Prop::TransitionProperty).is_some());
        assert!(get(&transition, Prop::TransitionDuration).is_some());
        assert!(get(&transition, Prop::TransitionDelay).is_some());
        let animation = expanded(expand_animation, "spin 1s linear infinite").expect("expands");
        assert!(get(&animation, Prop::AnimationName).is_some());
        assert!(get(&animation, Prop::AnimationFillMode).is_some());
        let font = expanded(expand_font, "italic bold 14px/1.5 Cantarell").expect("expands");
        assert_eq!(get(&font, Prop::FontSize), Some(&px(14.0)));
        assert!(get(&font, Prop::FontFamily).is_some());
        let decoration = expanded(expand_text_decoration, "underline wavy red").expect("expands");
        assert!(get(&decoration, Prop::TextDecorationLine).is_some());
        assert_eq!(
            get(&decoration, Prop::TextDecorationStyle),
            Some(&Value::Keyword(Keyword::Wavy))
        );
        let outline = expanded(expand_outline, "2px solid red").expect("expands");
        assert_eq!(get(&outline, Prop::OutlineWidth), Some(&px(2.0)));
        let image =
            expanded(expand_border_image, "url(\"b.png\") 30% / 2px round").expect("expands");
        assert!(get(&image, Prop::BorderImageSource).is_some());
        assert!(get(&image, Prop::BorderImageRepeat).is_some());
    }

    /// A failed `expect_delim('/')` consumes the token it rejected, so
    /// every optional `/` component must restore the parser state before
    /// the next component is read.
    #[test]
    fn an_absent_slash_component_does_not_swallow_the_next_token() {
        let font = expanded(expand_font, "14px Cantarell").expect("expands");
        assert_eq!(get(&font, Prop::FontSize), Some(&px(14.0)));
        assert_eq!(
            get(&font, Prop::LineHeight),
            Some(&Value::LineHeight(LineHeight::Normal))
        );
        assert!(get(&font, Prop::FontFamily).is_some());

        let image = expanded(expand_border_image, "url(\"b.png\") 30% round").expect("expands");
        assert_eq!(
            get(&image, Prop::BorderImageRepeat),
            Some(&Value::Repeat(RepeatStyle {
                x: Keyword::Round,
                y: Keyword::Round,
            }))
        );

        let background =
            expanded(expand_background, "url(\"b.png\") center no-repeat").expect("expands");
        assert_eq!(
            get(&background, Prop::BackgroundRepeat),
            Some(&Value::List(Rc::from(vec![Value::Repeat(RepeatStyle {
                x: Keyword::NoRepeat,
                y: Keyword::NoRepeat,
            })])))
        );

        // `border-radius` has nothing after the radii, so a trailing token
        // must survive to be rejected by `require_exhausted`.
        assert!(expanded(expand_border_radius, "5px solid").is_err());
    }

    #[test]
    fn shorthand_expansion_never_panics() {
        const EXPANDERS: &[ExpandFn] = &[
            expand_font,
            expand_text_decoration,
            expand_margin,
            expand_padding,
            expand_border_width,
            expand_border_style,
            expand_border_color,
            expand_border_top,
            expand_border_right,
            expand_border_bottom,
            expand_border_left,
            expand_border,
            expand_border_radius,
            expand_border_image,
            expand_outline,
            expand_background,
            expand_transition,
            expand_animation,
            expand_all,
        ];
        for input in crate::css::value::FUZZ_INPUTS {
            for expand in EXPANDERS {
                let _ = expanded(*expand, input);
            }
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::{expand_background, expand_border, expand_font};
    use crate::css::registry::{ExpandFn, Prop};
    use crate::css::value::Value;
    use crate::css::value::font::{FontStyle, FontWeight};
    use crate::css::value::image::Image;
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;

    fn expanded(expand: ExpandFn, text: &str) -> Result<Vec<(Prop, Value)>, ()> {
        let mut source = cssparser::ParserInput::new(text);
        let mut parser = cssparser::Parser::new(&mut source);
        let mut out: Vec<(Prop, Value)> = Vec::new();
        {
            let mut sink = |prop: Prop, value: Value| out.push((prop, value));
            expand(&mut parser, &mut sink)?;
        }
        parser.skip_whitespace();
        if parser.is_exhausted() {
            Ok(out)
        } else {
            Err(())
        }
    }

    fn get(pairs: &[(Prop, Value)], prop: Prop) -> Option<&Value> {
        pairs
            .iter()
            .find(|(key, _)| *key == prop)
            .map(|(_, value)| value)
    }

    // F54/F55: an explicit initial value in a `font` slot must be consumed,
    // not rejected -- rejecting it left the ident for `parse_font_size`,
    // which errored and dropped the whole declaration.
    #[test]
    fn the_font_shorthand_accepts_an_explicit_normal_400_or_100_percent() {
        for text in [
            "normal 12px sans-serif",
            "400 14px serif",
            "100% 14px serif",
            "normal normal normal 12px sans-serif",
            "normal 400 12px/1.5 sans-serif",
        ] {
            let pairs = expanded(expand_font, text).unwrap_or_else(|()| {
                panic!("`font: {text}` must expand");
            });
            assert_eq!(
                get(&pairs, Prop::FontStyle),
                Some(&Value::FontStyle(FontStyle::Normal)),
                "`font: {text}`"
            );
            assert_eq!(
                get(&pairs, Prop::FontWeight),
                Some(&Value::FontWeight(FontWeight::Absolute(400.0))),
                "`font: {text}`"
            );
        }
        // The non-initial forms must still work.
        let pairs = expanded(expand_font, "italic bold 16px cursive").expect("expands");
        assert_eq!(
            get(&pairs, Prop::FontStyle),
            Some(&Value::FontStyle(FontStyle::Italic))
        );
        assert_eq!(
            get(&pairs, Prop::FontWeight),
            Some(&Value::FontWeight(FontWeight::Absolute(700.0)))
        );
    }

    /// A bare `<percentage>` in a `font` shorthand is `font-stretch` only if
    /// a `<font-size>` still follows it; otherwise it *is* the size
    /// (CSS Fonts 3 §3.7).
    ///
    /// `font: 100% serif` was rejected outright: the stretch slot ate the
    /// `100%` and `parse_font_size` then found `serif`.
    ///
    /// Mutation check: delete the `ambiguous_percentage` retry in
    /// `expand_font` and the first two rows below stop expanding.
    #[test]
    fn a_bare_percentage_is_the_font_size_unless_a_size_follows_it() {
        let px = |value: f32| Some(Value::Length(Length::px(value)));
        let percent = |fraction: f32| Some(Value::Percentage(fraction));
        // (source, expected font-size, expected font-stretch)
        for (text, size, stretch) in [
            // Nothing follows, so the percentage is the size.
            (
                "100% serif",
                Some(Value::Length(Length::Percent(1.0))),
                percent(1.0),
            ),
            (
                "150% serif",
                Some(Value::Length(Length::Percent(1.5))),
                percent(1.0),
            ),
            // A size follows, so the percentage is the stretch.
            ("100% 14px serif", px(14.0), percent(1.0)),
            ("150% 14px serif", px(14.0), percent(1.5)),
            // A stretch *keyword* is never ambiguous, so the percentage that
            // follows it can only be the size.
            (
                "condensed 100% serif",
                Some(Value::Length(Length::Percent(1.0))),
                percent(0.75),
            ),
            // The unambiguous baseline, unchanged.
            ("normal 12px sans-serif", px(12.0), percent(1.0)),
        ] {
            let pairs = expanded(expand_font, text)
                .unwrap_or_else(|()| panic!("`font: {text}` must expand"));
            assert_eq!(
                get(&pairs, Prop::FontSize).cloned(),
                size,
                "`font: {text}` font-size"
            );
            assert_eq!(
                get(&pairs, Prop::FontStretch).cloned(),
                stretch,
                "`font: {text}` font-stretch"
            );
            // F16: the two spellings always agree.
            assert_eq!(
                get(&pairs, Prop::FontWidth),
                get(&pairs, Prop::FontStretch),
                "`font: {text}`"
            );
        }
        // Still invalid either way: no family, and no size at all.
        assert!(expanded(expand_font, "100% 14px").is_err());
        assert!(expanded(expand_font, "100%").is_err());
    }

    // F35/F36: `small-caps` followed by another ident used to poison the
    // whole shorthand, because the variant run hard-errored on `bold`.
    #[test]
    fn small_caps_may_be_followed_by_another_font_component() {
        for text in [
            "italic small-caps bold 16px/2 cursive",
            "small-caps bold 12px serif",
            "small-caps 12px serif",
        ] {
            assert!(
                expanded(expand_font, text).is_ok(),
                "`font: {text}` must expand"
            );
        }
    }

    // F16: `font` resets `font-width`, which `text.rs` reads before
    // `font-stretch`.
    #[test]
    fn the_font_shorthand_resets_font_width_as_well_as_font_stretch() {
        let pairs = expanded(expand_font, "14px Cantarell").expect("expands");
        assert_eq!(
            get(&pairs, Prop::FontWidth),
            Some(&Value::Percentage(1.0)),
            "font-width must be reset to its initial 100%"
        );
        assert_eq!(
            get(&pairs, Prop::FontStretch),
            Some(&Value::Percentage(1.0))
        );
        let condensed = expanded(expand_font, "condensed 14px Cantarell").expect("expands");
        assert_eq!(
            get(&condensed, Prop::FontWidth),
            get(&condensed, Prop::FontStretch),
            "the two spellings must always agree"
        );
    }

    // F17: `border` resets `border-image-*`.
    #[test]
    fn the_border_shorthand_resets_every_border_image_longhand() {
        let pairs = expanded(expand_border, "1px solid red").expect("expands");
        for prop in [
            Prop::BorderImageSource,
            Prop::BorderImageSlice,
            Prop::BorderImageWidth,
            Prop::BorderImageRepeat,
        ] {
            assert_eq!(
                get(&pairs, prop),
                Some(&prop.initial()),
                "{} must be reset to its initial value",
                prop.name()
            );
        }
    }

    // F52/F53: one `<box>` in `background` sets origin *and* clip.
    #[test]
    fn one_background_box_keyword_sets_both_origin_and_clip() {
        let pairs = expanded(expand_background, "url(\"a.png\") content-box").expect("expands");
        assert_eq!(
            get(&pairs, Prop::BackgroundOrigin),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::ContentBox
            )])))
        );
        assert_eq!(
            get(&pairs, Prop::BackgroundClip),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::ContentBox
            )]))),
            "one <box> sets both longhands"
        );
        // Two boxes still set them separately, origin first.
        let two =
            expanded(expand_background, "url(\"a.png\") content-box padding-box").expect("expands");
        assert_eq!(
            get(&two, Prop::BackgroundOrigin),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::ContentBox
            )])))
        );
        assert_eq!(
            get(&two, Prop::BackgroundClip),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::PaddingBox
            )])))
        );
    }

    // F51: a layer that carries a colour *and* other components keeps them.
    #[test]
    fn a_coloured_layer_that_also_carries_components_keeps_them() {
        let pairs = expanded(expand_background, "red content-box").expect("expands");
        assert_eq!(
            get(&pairs, Prop::BackgroundOrigin),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::ContentBox
            )]))),
            "the layer's own origin must survive"
        );
        assert_eq!(
            get(&pairs, Prop::BackgroundClip),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::ContentBox
            )])))
        );
        match get(&pairs, Prop::BackgroundImage) {
            Some(Value::List(items)) => {
                assert_eq!(items.len(), 1);
                assert!(matches!(items[0], Value::Image(Image::None)));
            }
            other => panic!("expected a one-entry image list, got {other:?}"),
        }
        // A bare colour is still nothing but a colour: no layer.
        let bare = expanded(expand_background, "red").expect("expands");
        assert_eq!(
            get(&bare, Prop::BackgroundOrigin),
            Some(&Value::List(std::rc::Rc::from(vec![Value::Keyword(
                Keyword::PaddingBox
            )])))
        );
    }
}
