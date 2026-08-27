//! `<image>`: gradients, `url()`, `image()`, `cross-fade()` and GTK's
//! `-gtk-icontheme()`/`-gtk-recolor()`/`-gtk-scaled()`.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::parse_angle;
use super::color::ColorValue;
use super::length::Length;

/// A `<position>`: `background-position` and `transform-origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    /// Horizontal offset from the origin box's left edge.
    pub x: Length,
    /// Vertical offset from its top edge.
    pub y: Length,
    /// Optional z component (`transform-origin` only).
    pub z: Option<Length>,
}

impl Position {
    /// `50% 50%`.
    #[must_use]
    pub fn center() -> Position {
        Position {
            x: Length::Percent(0.5),
            y: Length::Percent(0.5),
            z: None,
        }
    }

    /// Parse `[<length-percentage> | left | center | right]
    /// [<length-percentage> | top | center | bottom]? <length>?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Position, ()> {
        let first = parse_position_component(input)?;
        let state = input.state();
        let second = match parse_position_component(input) {
            Ok(component) => Some(component),
            Err(()) => {
                input.reset(&state);
                None
            }
        };
        let (x, y) = match (first, second) {
            (Component::Axis(Axis::Vertical, value), None) => (Length::Percent(0.5), value),
            (Component::Axis(_, value), None) | (Component::Free(value), None) => {
                (value, Length::Percent(0.5))
            }
            (a, Some(b)) => resolve_pair(a, b)?,
        };
        let state = input.state();
        let z = match Length::parse(input) {
            Ok(length) => Some(length),
            Err(()) => {
                input.reset(&state);
                None
            }
        };
        Ok(Position { x, y, z })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug)]
enum Component {
    /// A keyword that pins an axis (`left`/`right`/`top`/`bottom`).
    Axis(Axis, Length),
    /// `center`, a length or a percentage: usable on either axis.
    Free(Length),
}

fn parse_position_component(input: &mut Parser<'_, '_>) -> Result<Component, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    if let Token::Ident(ref name) = token {
        if name.eq_ignore_ascii_case("left") {
            return Ok(Component::Axis(Axis::Horizontal, Length::Percent(0.0)));
        }
        if name.eq_ignore_ascii_case("right") {
            return Ok(Component::Axis(Axis::Horizontal, Length::Percent(1.0)));
        }
        if name.eq_ignore_ascii_case("top") {
            return Ok(Component::Axis(Axis::Vertical, Length::Percent(0.0)));
        }
        if name.eq_ignore_ascii_case("bottom") {
            return Ok(Component::Axis(Axis::Vertical, Length::Percent(1.0)));
        }
        if name.eq_ignore_ascii_case("center") {
            return Ok(Component::Free(Length::Percent(0.5)));
        }
        return Err(());
    }
    input.reset(&state);
    Length::parse(input).map(Component::Free)
}

fn resolve_pair(a: Component, b: Component) -> Result<(Length, Length), ()> {
    match (a, b) {
        (Component::Axis(Axis::Vertical, y), Component::Axis(Axis::Horizontal, x))
        | (Component::Axis(Axis::Horizontal, x), Component::Axis(Axis::Vertical, y)) => Ok((x, y)),
        (Component::Axis(Axis::Horizontal, x), Component::Free(y))
        | (Component::Free(x), Component::Free(y)) => Ok((x, y)),
        (Component::Free(x), Component::Axis(Axis::Vertical, y)) => Ok((x, y)),
        (Component::Axis(Axis::Vertical, y), Component::Free(x)) => Ok((x, y)),
        _ => Err(()),
    }
}

/// A gradient's colour stop.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorStop {
    /// The stop colour.
    pub color: ColorValue,
    /// Explicit position along the gradient line.
    pub position: Option<Length>,
    /// Interpolation hint appearing *before* this stop.
    pub hint: Option<Length>,
}

/// Which side or corner a `to ...` linear gradient points at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SideOrCorner {
    /// `to top`.
    Top,
    /// `to right`.
    Right,
    /// `to bottom`.
    Bottom,
    /// `to left`.
    Left,
    /// `to top left`.
    TopLeft,
    /// `to top right`.
    TopRight,
    /// `to bottom left`.
    BottomLeft,
    /// `to bottom right`.
    BottomRight,
}

/// A linear gradient's direction.
#[derive(Clone, Debug, PartialEq)]
pub enum LinearDirection {
    /// An explicit angle, in degrees, `0deg` pointing up.
    Angle(f32),
    /// A `to <side-or-corner>` keyword.
    Side(SideOrCorner),
}

/// A radial gradient's shape.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RadialShape {
    /// `circle`.
    Circle,
    /// `ellipse`.
    Ellipse,
}

/// A radial gradient's extent.
#[derive(Clone, Debug, PartialEq)]
pub enum RadialExtent {
    /// `closest-side`.
    ClosestSide,
    /// `closest-corner`.
    ClosestCorner,
    /// `farthest-side`.
    FarthestSide,
    /// `farthest-corner` (the default).
    FarthestCorner,
    /// Explicit radii.
    Explicit(Length, Length),
}

/// Which family of gradient this is.
#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    /// `linear-gradient()`.
    Linear {
        /// Direction of the gradient line.
        direction: LinearDirection,
    },
    /// `radial-gradient()`.
    Radial {
        /// Circle or ellipse.
        shape: RadialShape,
        /// How far the gradient reaches.
        extent: RadialExtent,
        /// The centre.
        position: Position,
    },
    /// `conic-gradient()`.
    Conic {
        /// Starting angle in degrees.
        from_angle: f32,
        /// The centre.
        position: Position,
    },
}

/// A parsed gradient.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// Which gradient family.
    pub kind: GradientKind,
    /// `repeating-*` form.
    pub repeating: bool,
    /// Colour stops, in order, at least two.
    pub stops: Rc<[ColorStop]>,
    /// `in <space>` interpolation space, if given.
    pub interpolation: Option<super::color::ColorSpace>,
}

/// A `-gtk-recolor()` / `-gtk-icon-palette` palette: `name <color>` pairs.
pub type IconPalette = Rc<[(Rc<str>, ColorValue)]>;

/// A GTK image function, stored for Part 4 of the milestone chain (icons
/// are drawn in M4, not M2).
#[derive(Clone, Debug, PartialEq)]
pub enum IconRef {
    /// `-gtk-icontheme(name)`.
    Theme {
        /// Icon name.
        name: Rc<str>,
    },
    /// `-gtk-recolor(url, palette?)`.
    Recolor {
        /// Source URL.
        url: Rc<str>,
        /// Optional palette override.
        palette: Option<IconPalette>,
    },
    /// `-gtk-scaled(lo, hi)`.
    Scaled {
        /// Normal-resolution image.
        lo: Rc<Image>,
        /// Hi-DPI image.
        hi: Rc<Image>,
    },
}

/// A parsed `<image>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    /// `none`.
    None,
    /// `url(...)` -- decoded lazily; a URL that fails to decode is
    /// recorded-unresolved and paints nothing, it is not an error.
    Url(Rc<str>),
    /// `image(<color>)`.
    Solid(ColorValue),
    /// Any gradient.
    Gradient(Rc<Gradient>),
    /// `cross-fade()`; `None` shares are auto-distributed.
    CrossFade(Rc<[(Option<f32>, Image)]>),
    /// A GTK image function.
    Icon(Rc<IconRef>),
}

impl Image {
    /// Parse a whole `<image>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Image, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("none") => Ok(Image::None),
            Token::UnquotedUrl(ref url) => Ok(Image::Url(Rc::from(url.as_ref()))),
            Token::Function(ref name) => {
                let name = name.clone();
                parse_image_function(&name, input)
            }
            _ => Err(()),
        }
    }
}

fn parse_image_function(name: &str, input: &mut Parser<'_, '_>) -> Result<Image, ()> {
    let lower = name.to_ascii_lowercase();
    let (repeating, family) = match lower.strip_prefix("repeating-") {
        Some(rest) => (true, rest.to_string()),
        None => (false, lower.clone()),
    };
    match family.as_str() {
        "url" => nested(input, |inner| {
            let url = inner.expect_string().map_err(|_| ())?.as_ref().to_string();
            require_exhausted(inner)?;
            Ok(Image::Url(Rc::from(url.as_str())))
        }),
        "image" => nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            require_exhausted(inner)?;
            Ok(Image::Solid(color))
        }),
        "linear-gradient" => nested_gradient(input, repeating, GradientFamily::Linear),
        "radial-gradient" => nested_gradient(input, repeating, GradientFamily::Radial),
        "conic-gradient" => nested_gradient(input, repeating, GradientFamily::Conic),
        "cross-fade" => nested(input, |inner| {
            let layers = inner
                .parse_comma_separated(|argument| match parse_cross_fade_layer(argument) {
                    Ok(layer) => Ok(layer),
                    Err(()) => Err(argument.new_custom_error::<(), ()>(())),
                })
                .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
            if layers.is_empty() {
                return Err(());
            }
            Ok(Image::CrossFade(layers.into()))
        }),
        "-gtk-icontheme" => nested(input, |inner| {
            let icon = inner
                .expect_ident_or_string()
                .map_err(|_| ())?
                .as_ref()
                .to_string();
            require_exhausted(inner)?;
            Ok(Image::Icon(Rc::new(IconRef::Theme {
                name: Rc::from(icon.as_str()),
            })))
        }),
        "-gtk-recolor" => nested(input, |inner| {
            let url = match Image::parse(inner)? {
                Image::Url(url) => url,
                _ => return Err(()),
            };
            let palette = if inner.expect_comma().is_ok() {
                Some(parse_icon_palette(inner)?)
            } else {
                None
            };
            require_exhausted(inner)?;
            Ok(Image::Icon(Rc::new(IconRef::Recolor { url, palette })))
        }),
        "-gtk-scaled" => nested(input, |inner| {
            let lo = Image::parse(inner)?;
            inner.expect_comma().map_err(|_| ())?;
            let hi = Image::parse(inner)?;
            require_exhausted(inner)?;
            Ok(Image::Icon(Rc::new(IconRef::Scaled {
                lo: Rc::new(lo),
                hi: Rc::new(hi),
            })))
        }),
        _ => Err(()),
    }
}

/// `name <color>` pairs, comma separated -- also `-gtk-icon-palette`'s value.
pub(crate) fn parse_icon_palette(input: &mut Parser<'_, '_>) -> Result<IconPalette, ()> {
    let entries = input
        .parse_comma_separated(|argument| {
            let parsed = (|| {
                let name = argument
                    .expect_ident()
                    .map_err(|_| ())?
                    .as_ref()
                    .to_string();
                let color = ColorValue::parse(argument)?;
                Ok::<(Rc<str>, ColorValue), ()>((Rc::from(name.as_str()), color))
            })();
            match parsed {
                Ok(entry) => Ok(entry),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            }
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    Ok(entries.into())
}

fn parse_cross_fade_layer(input: &mut Parser<'_, '_>) -> Result<(Option<f32>, Image), ()> {
    let state = input.state();
    let share = match input.next() {
        Ok(Token::Percentage { unit_value, .. }) if unit_value.is_finite() => Some(*unit_value),
        _ => None,
    };
    if share.is_none() {
        input.reset(&state);
    }
    let image = Image::parse(input)?;
    Ok((share, image))
}

#[derive(Copy, Clone)]
enum GradientFamily {
    Linear,
    Radial,
    Conic,
}

fn nested_gradient(
    input: &mut Parser<'_, '_>,
    repeating: bool,
    family: GradientFamily,
) -> Result<Image, ()> {
    input
        .parse_nested_block(
            |inner| match parse_gradient_body(inner, repeating, family) {
                Ok(image) => Ok(image),
                Err(()) => Err(inner.new_custom_error::<(), ()>(())),
            },
        )
        .map_err(|_: cssparser::ParseError<'_, ()>| ())
}

fn parse_gradient_body(
    input: &mut Parser<'_, '_>,
    repeating: bool,
    family: GradientFamily,
) -> Result<Image, ()> {
    let kind = match family {
        GradientFamily::Linear => GradientKind::Linear {
            direction: parse_linear_direction(input)?,
        },
        GradientFamily::Radial => parse_radial_prelude(input)?,
        GradientFamily::Conic => parse_conic_prelude(input)?,
    };
    let stops = parse_stops(input)?;
    if stops.len() < 2 {
        return Err(());
    }
    require_exhausted(input)?;
    Ok(Image::Gradient(Rc::new(Gradient {
        kind,
        repeating,
        stops: stops.into(),
        interpolation: None,
    })))
}

/// `to <side-or-corner>` / `<angle>` / nothing (defaults to `to bottom`).
/// A trailing comma is consumed when a prelude was present.
fn parse_linear_direction(input: &mut Parser<'_, '_>) -> Result<LinearDirection, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    if let Token::Ident(ref name) = token
        && name.eq_ignore_ascii_case("to")
    {
        let side = parse_side_or_corner(input)?;
        input.expect_comma().map_err(|_| ())?;
        return Ok(LinearDirection::Side(side));
    }
    input.reset(&state);
    if let Ok(angle) = parse_angle(input) {
        input.expect_comma().map_err(|_| ())?;
        return Ok(LinearDirection::Angle(angle));
    }
    input.reset(&state);
    Ok(LinearDirection::Side(SideOrCorner::Bottom))
}

fn parse_side_or_corner(input: &mut Parser<'_, '_>) -> Result<SideOrCorner, ()> {
    let first = input
        .expect_ident()
        .map_err(|_| ())?
        .as_ref()
        .to_ascii_lowercase();
    let state = input.state();
    let second = match input.expect_ident() {
        Ok(name) => Some(name.as_ref().to_ascii_lowercase()),
        Err(_) => {
            input.reset(&state);
            None
        }
    };
    let mut sides = [first.as_str(), second.as_deref().unwrap_or("")];
    sides.sort_unstable();
    match (sides[0], sides[1]) {
        ("", "top") => Ok(SideOrCorner::Top),
        ("", "right") => Ok(SideOrCorner::Right),
        ("", "bottom") => Ok(SideOrCorner::Bottom),
        ("", "left") => Ok(SideOrCorner::Left),
        ("left", "top") => Ok(SideOrCorner::TopLeft),
        ("right", "top") => Ok(SideOrCorner::TopRight),
        ("bottom", "left") => Ok(SideOrCorner::BottomLeft),
        ("bottom", "right") => Ok(SideOrCorner::BottomRight),
        _ => Err(()),
    }
}

fn parse_radial_prelude(input: &mut Parser<'_, '_>) -> Result<GradientKind, ()> {
    let mut shape = RadialShape::Ellipse;
    let mut extent = RadialExtent::FarthestCorner;
    let mut position = Position::center();
    let mut radii: Vec<Length> = Vec::new();
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        match token {
            Token::Comma => break,
            Token::Ident(ref name) if name.eq_ignore_ascii_case("circle") => {
                shape = RadialShape::Circle;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("ellipse") => {
                shape = RadialShape::Ellipse;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("at") => {
                position = Position::parse(input)?;
            }
            Token::Ident(ref name) => {
                extent = match name.to_ascii_lowercase().as_str() {
                    "closest-side" => RadialExtent::ClosestSide,
                    "closest-corner" => RadialExtent::ClosestCorner,
                    "farthest-side" => RadialExtent::FarthestSide,
                    "farthest-corner" => RadialExtent::FarthestCorner,
                    _ => {
                        input.reset(&state);
                        break;
                    }
                };
            }
            _ => {
                input.reset(&state);
                match Length::parse(input) {
                    Ok(length) => radii.push(length),
                    Err(()) => {
                        input.reset(&state);
                        break;
                    }
                }
            }
        }
    }
    if radii.len() == 1 {
        extent = RadialExtent::Explicit(radii[0].clone(), radii[0].clone());
    } else if radii.len() >= 2 {
        extent = RadialExtent::Explicit(radii[0].clone(), radii[1].clone());
    }
    Ok(GradientKind::Radial {
        shape,
        extent,
        position,
    })
}

fn parse_conic_prelude(input: &mut Parser<'_, '_>) -> Result<GradientKind, ()> {
    let mut from_angle = 0.0;
    let mut position = Position::center();
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        match token {
            Token::Comma => break,
            Token::Ident(ref name) if name.eq_ignore_ascii_case("from") => {
                from_angle = parse_angle(input)?;
            }
            Token::Ident(ref name) if name.eq_ignore_ascii_case("at") => {
                position = Position::parse(input)?;
            }
            _ => {
                input.reset(&state);
                break;
            }
        }
    }
    Ok(GradientKind::Conic {
        from_angle,
        position,
    })
}

/// `<color-stop-list>`: stops, optional positions (one or two per stop) and
/// bare interpolation hints between them.
fn parse_stops(input: &mut Parser<'_, '_>) -> Result<Vec<ColorStop>, ()> {
    let mut stops: Vec<ColorStop> = Vec::new();
    let mut pending_hint: Option<Length> = None;
    loop {
        let state = input.state();
        // A bare length between two stops is an interpolation hint.
        if let Ok(hint) = Length::parse(input) {
            let comma = input.expect_comma().is_ok();
            if comma && !stops.is_empty() {
                pending_hint = Some(hint);
                continue;
            }
            input.reset(&state);
        } else {
            input.reset(&state);
        }
        let color = ColorValue::parse(input)?;
        let mut positions: Vec<Length> = Vec::new();
        for _ in 0..2 {
            let state = input.state();
            match Length::parse(input) {
                Ok(length) => positions.push(length),
                Err(()) => {
                    input.reset(&state);
                    break;
                }
            }
        }
        if positions.is_empty() {
            stops.push(ColorStop {
                color,
                position: None,
                hint: pending_hint.take(),
            });
        } else {
            for position in positions {
                stops.push(ColorStop {
                    color: color.clone(),
                    position: Some(position),
                    hint: pending_hint.take(),
                });
            }
        }
        if input.expect_comma().is_err() {
            return Ok(stops);
        }
    }
}

fn nested<T>(
    input: &mut Parser<'_, '_>,
    body: fn(&mut Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    input
        .parse_nested_block(|inner| match body(inner) {
            Ok(value) => Ok(value),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())
}

fn require_exhausted(input: &mut Parser<'_, '_>) -> Result<(), ()> {
    input.skip_whitespace();
    if input.is_exhausted() {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Gradient, GradientKind, Image, LinearDirection, Position, RadialExtent, SideOrCorner,
    };
    use crate::css::value::color::ColorValue;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    fn image(text: &str) -> Option<Image> {
        parse_entirely_with(text, Image::parse).ok()
    }

    fn gradient(text: &str) -> Option<Gradient> {
        match image(text)? {
            Image::Gradient(gradient) => Some((*gradient).clone()),
            _ => None,
        }
    }

    #[test]
    fn adwaitas_button_gradient_parses_with_its_stop_positions() {
        // The exact declaration the offscreen pixel gate pins.
        let gradient = gradient("linear-gradient(to top, #f6f5f4 2px, #fbfafa)")
            .expect("Adwaita's button gradient parses");
        assert!(!gradient.repeating);
        assert_eq!(
            gradient.kind,
            GradientKind::Linear {
                direction: LinearDirection::Side(SideOrCorner::Top)
            }
        );
        assert_eq!(gradient.stops.len(), 2);
        assert_eq!(gradient.stops[0].position, Some(Length::px(2.0)));
        assert_eq!(gradient.stops[1].position, None);
    }

    #[test]
    fn every_gradient_kind_and_the_repeating_forms_parse() {
        assert!(gradient("linear-gradient(45deg, red, blue)").is_some());
        assert!(gradient("linear-gradient(to bottom right, red, blue)").is_some());
        assert!(gradient("repeating-linear-gradient(red, blue)").is_some());
        assert!(gradient("radial-gradient(circle closest-side at 10px 20px, red, blue)").is_some());
        assert!(
            gradient("radial-gradient(farthest-corner, red 0%, rgba(53,132,228,0) 100%)").is_some()
        );
        assert!(gradient("repeating-radial-gradient(red, blue)").is_some());
        assert!(gradient("conic-gradient(from 90deg at center, red, blue)").is_some());
        assert!(gradient("repeating-conic-gradient(red, blue)").is_some());
        // A gradient needs at least two stops.
        assert!(gradient("linear-gradient(red)").is_none());
    }

    #[test]
    fn a_default_linear_gradient_points_down_and_a_radial_one_is_centred() {
        let linear = gradient("linear-gradient(red, blue)").expect("parses");
        assert_eq!(
            linear.kind,
            GradientKind::Linear {
                direction: LinearDirection::Side(SideOrCorner::Bottom)
            }
        );
        let radial = gradient("radial-gradient(red, blue)").expect("parses");
        match radial.kind {
            GradientKind::Radial {
                extent, position, ..
            } => {
                assert_eq!(extent, RadialExtent::FarthestCorner);
                assert_eq!(position, Position::center());
            }
            other => panic!("expected a radial gradient, got {other:?}"),
        }
    }

    #[test]
    fn interpolation_hints_attach_to_the_stop_that_follows_them() {
        let gradient = gradient("linear-gradient(red, 30%, blue)").expect("parses");
        assert_eq!(gradient.stops.len(), 2);
        assert_eq!(gradient.stops[1].hint, Some(Length::Percent(0.3)));
    }

    #[test]
    fn a_double_position_stop_expands_to_two_stops() {
        let gradient = gradient("linear-gradient(red 0% 50%, blue)").expect("parses");
        assert_eq!(gradient.stops.len(), 3);
        assert_eq!(gradient.stops[0].position, Some(Length::Percent(0.0)));
        assert_eq!(gradient.stops[1].position, Some(Length::Percent(0.5)));
    }

    #[test]
    fn the_non_gradient_image_forms_parse() {
        assert_eq!(image("none"), Some(Image::None));
        assert_eq!(
            image("url(\"icon.png\")"),
            Some(Image::Url("icon.png".into()))
        );
        assert_eq!(
            image("image(#e8e6e3)"),
            Some(Image::Solid(
                ColorValue::parse(&mut cssparser::Parser::new(
                    &mut cssparser::ParserInput::new("#e8e6e3")
                ))
                .expect("colour parses")
            ))
        );
        assert!(matches!(
            image("cross-fade(20% url(\"a.png\"), url(\"b.png\"))"),
            Some(Image::CrossFade(_))
        ));
        assert!(matches!(
            image("-gtk-icontheme(\"go-next\")"),
            Some(Image::Icon(_))
        ));
        assert!(matches!(
            image("-gtk-recolor(url(\"a.svg\"))"),
            Some(Image::Icon(_))
        ));
        assert!(matches!(
            image("-gtk-scaled(url(\"a.png\"), url(\"a@2.png\"))"),
            Some(Image::Icon(_))
        ));
        // GTK requires quotes around url() arguments; an unquoted url token
        // is still accepted, because cssparser tokenizes it for free.
        assert_eq!(image("url(icon.png)"), Some(Image::Url("icon.png".into())));
    }

    #[test]
    fn positions_accept_keywords_lengths_and_the_three_value_form() {
        let parse = |text: &str| parse_entirely_with(text, Position::parse).ok();
        assert_eq!(parse("center"), Some(Position::center()));
        assert_eq!(
            parse("left top"),
            Some(Position {
                x: Length::Percent(0.0),
                y: Length::Percent(0.0),
                z: None
            })
        );
        assert_eq!(
            parse("10px 20px"),
            Some(Position {
                x: Length::px(10.0),
                y: Length::px(20.0),
                z: None
            })
        );
        assert_eq!(
            parse("50% 50% 3px"),
            Some(Position {
                x: Length::Percent(0.5),
                y: Length::Percent(0.5),
                z: Some(Length::px(3.0))
            })
        );
        // A single value leaves the other axis centred.
        assert_eq!(
            parse("right"),
            Some(Position {
                x: Length::Percent(1.0),
                y: Length::Percent(0.5),
                z: None
            })
        );
    }

    #[test]
    fn image_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Image::parse);
            let _ = parse_entirely_with(input, Position::parse);
        }
    }
}
