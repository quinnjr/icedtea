//! Font-family, weight, style, size, stretch, line-height, feature and
//! variation values.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::parse_angle;
use super::length::Length;

/// The engine's `medium` font size, matching M1's `DEFAULT_FONT_SIZE`.
pub const MEDIUM_FONT_SIZE_PX: f32 = 14.0;

/// A CSS generic family.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GenericFamily {
    /// `serif`.
    Serif,
    /// `sans-serif`.
    SansSerif,
    /// `monospace`.
    Monospace,
    /// `cursive`.
    Cursive,
    /// `fantasy`.
    Fantasy,
    /// `system-ui`.
    SystemUi,
}

impl GenericFamily {
    fn from_str_ascii_ci(name: &str) -> Option<GenericFamily> {
        const GENERICS: &[(&str, GenericFamily)] = &[
            ("serif", GenericFamily::Serif),
            ("sans-serif", GenericFamily::SansSerif),
            ("monospace", GenericFamily::Monospace),
            ("cursive", GenericFamily::Cursive),
            ("fantasy", GenericFamily::Fantasy),
            ("system-ui", GenericFamily::SystemUi),
        ];
        GENERICS
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, generic)| *generic)
    }
}

/// One entry of a `font-family` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontFamily {
    /// A concrete family name.
    Named(Rc<str>),
    /// A generic family.
    Generic(GenericFamily),
}

/// `font-weight`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FontWeight {
    /// An absolute weight in `1..=1000`.
    Absolute(f32),
    /// One step bolder than the parent's.
    Bolder,
    /// One step lighter than the parent's.
    Lighter,
}

/// `font-style`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FontStyle {
    /// Upright.
    Normal,
    /// Italic.
    Italic,
    /// Oblique, in degrees; bare `oblique` is `14`.
    Oblique(f32),
}

/// `line-height`.
#[derive(Clone, Debug, PartialEq)]
pub enum LineHeight {
    /// The face's own line height.
    Normal,
    /// A multiple of the font size.
    Number(f32),
    /// An explicit length or percentage of the font size.
    Length(Length),
}

/// One `font-feature-settings` entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeatureSetting {
    /// The four-byte OpenType tag.
    pub tag: [u8; 4],
    /// The feature value; `on` == 1, `off` == 0.
    pub value: i32,
}

/// One `font-variation-settings` entry.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VariationSetting {
    /// The four-byte axis tag.
    pub tag: [u8; 4],
    /// The axis value.
    pub value: f32,
}

bitflags::bitflags! {
    /// One flag set covering `font-variant` and every `font-variant-*`
    /// longhand, so all of them share a single `Value` variant.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct FontVariantFlags: u32 {
        /// `small-caps`.
        const SMALL_CAPS = 1 << 0;
        /// `all-small-caps`.
        const ALL_SMALL_CAPS = 1 << 1;
        /// `petite-caps`.
        const PETITE_CAPS = 1 << 2;
        /// `all-petite-caps`.
        const ALL_PETITE_CAPS = 1 << 3;
        /// `unicase`.
        const UNICASE = 1 << 4;
        /// `titling-caps`.
        const TITLING_CAPS = 1 << 5;
        /// `no-common-ligatures`.
        const NO_COMMON_LIGATURES = 1 << 6;
        /// `discretionary-ligatures`.
        const DISCRETIONARY_LIGATURES = 1 << 7;
        /// `historical-ligatures`.
        const HISTORICAL_LIGATURES = 1 << 8;
        /// `no-contextual`.
        const NO_CONTEXTUAL = 1 << 9;
        /// `sub`.
        const SUB = 1 << 10;
        /// `super`.
        const SUPER = 1 << 11;
        /// `lining-nums`.
        const LINING_NUMS = 1 << 12;
        /// `oldstyle-nums`.
        const OLDSTYLE_NUMS = 1 << 13;
        /// `proportional-nums`.
        const PROPORTIONAL_NUMS = 1 << 14;
        /// `tabular-nums`.
        const TABULAR_NUMS = 1 << 15;
        /// `diagonal-fractions`.
        const DIAGONAL_FRACTIONS = 1 << 16;
        /// `stacked-fractions`.
        const STACKED_FRACTIONS = 1 << 17;
        /// `ordinal`.
        const ORDINAL = 1 << 18;
        /// `slashed-zero`.
        const SLASHED_ZERO = 1 << 19;
        /// `historical-forms`.
        const HISTORICAL_FORMS = 1 << 20;
        /// `ruby`.
        const RUBY = 1 << 21;
        /// `jis78`.
        const JIS78 = 1 << 22;
        /// `jis83`.
        const JIS83 = 1 << 23;
        /// `jis90`.
        const JIS90 = 1 << 24;
        /// `jis04`.
        const JIS04 = 1 << 25;
        /// `simplified`.
        const SIMPLIFIED = 1 << 26;
        /// `traditional`.
        const TRADITIONAL = 1 << 27;
        /// `full-width` (east-asian).
        const FULL_WIDTH_EA = 1 << 28;
        /// `proportional-width` (east-asian).
        const PROPORTIONAL_WIDTH_EA = 1 << 29;
    }
}

/// Every `font-variant-*` keyword and the flag it sets.
const VARIANT_KEYWORDS: &[(&str, FontVariantFlags)] = &[
    ("small-caps", FontVariantFlags::SMALL_CAPS),
    ("all-small-caps", FontVariantFlags::ALL_SMALL_CAPS),
    ("petite-caps", FontVariantFlags::PETITE_CAPS),
    ("all-petite-caps", FontVariantFlags::ALL_PETITE_CAPS),
    ("unicase", FontVariantFlags::UNICASE),
    ("titling-caps", FontVariantFlags::TITLING_CAPS),
    ("no-common-ligatures", FontVariantFlags::NO_COMMON_LIGATURES),
    (
        "discretionary-ligatures",
        FontVariantFlags::DISCRETIONARY_LIGATURES,
    ),
    (
        "historical-ligatures",
        FontVariantFlags::HISTORICAL_LIGATURES,
    ),
    ("no-contextual", FontVariantFlags::NO_CONTEXTUAL),
    ("sub", FontVariantFlags::SUB),
    ("super", FontVariantFlags::SUPER),
    ("lining-nums", FontVariantFlags::LINING_NUMS),
    ("oldstyle-nums", FontVariantFlags::OLDSTYLE_NUMS),
    ("proportional-nums", FontVariantFlags::PROPORTIONAL_NUMS),
    ("tabular-nums", FontVariantFlags::TABULAR_NUMS),
    ("diagonal-fractions", FontVariantFlags::DIAGONAL_FRACTIONS),
    ("stacked-fractions", FontVariantFlags::STACKED_FRACTIONS),
    ("ordinal", FontVariantFlags::ORDINAL),
    ("slashed-zero", FontVariantFlags::SLASHED_ZERO),
    ("historical-forms", FontVariantFlags::HISTORICAL_FORMS),
    ("ruby", FontVariantFlags::RUBY),
    ("jis78", FontVariantFlags::JIS78),
    ("jis83", FontVariantFlags::JIS83),
    ("jis90", FontVariantFlags::JIS90),
    ("jis04", FontVariantFlags::JIS04),
    ("simplified", FontVariantFlags::SIMPLIFIED),
    ("traditional", FontVariantFlags::TRADITIONAL),
    ("full-width", FontVariantFlags::FULL_WIDTH_EA),
    (
        "proportional-width",
        FontVariantFlags::PROPORTIONAL_WIDTH_EA,
    ),
];

/// `normal | <keyword>+`, restricted to `allowed` so each longhand rejects
/// the other longhands' keywords.
pub fn parse_variant_flags(
    input: &mut Parser<'_, '_>,
    allowed: FontVariantFlags,
) -> Result<FontVariantFlags, ()> {
    let mut flags = FontVariantFlags::empty();
    let mut count = 0_u32;
    loop {
        let state = input.state();
        let name = match input.expect_ident() {
            Ok(name) => name.as_ref().to_string(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        if name.eq_ignore_ascii_case("normal") {
            if count > 0 {
                return Err(());
            }
            count += 1;
            continue;
        }
        let flag = VARIANT_KEYWORDS
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, flag)| *flag)
            .ok_or(())?;
        if !allowed.contains(flag) || flags.contains(flag) {
            return Err(());
        }
        flags |= flag;
        count += 1;
    }
    if count == 0 { Err(()) } else { Ok(flags) }
}

/// `<family-name>#`.
pub fn parse_family_list(input: &mut Parser<'_, '_>) -> Result<Rc<[FontFamily]>, ()> {
    let families = input
        .parse_comma_separated(|argument| match parse_one_family(argument) {
            Ok(family) => Ok(family),
            Err(()) => Err(argument.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    if families.is_empty() {
        return Err(());
    }
    Ok(families.into())
}

fn parse_one_family(input: &mut Parser<'_, '_>) -> Result<FontFamily, ()> {
    let state = input.state();
    if let Ok(quoted) = input.expect_string() {
        return Ok(FontFamily::Named(Rc::from(quoted.as_ref())));
    }
    input.reset(&state);
    let first = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    if let Some(generic) = GenericFamily::from_str_ascii_ci(&first) {
        return Ok(FontFamily::Generic(generic));
    }
    // An unquoted family may be several idents: `Adwaita Sans`.
    let mut name = first;
    loop {
        let state = input.state();
        match input.expect_ident() {
            Ok(part) => {
                name.push(' ');
                name.push_str(part.as_ref());
            }
            Err(_) => {
                input.reset(&state);
                break;
            }
        }
    }
    Ok(FontFamily::Named(Rc::from(name.as_str())))
}

impl FontWeight {
    /// `normal | bold | bolder | lighter | <number [1,1000]>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<FontWeight, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) => {
                if name.eq_ignore_ascii_case("normal") {
                    Ok(FontWeight::Absolute(400.0))
                } else if name.eq_ignore_ascii_case("bold") {
                    Ok(FontWeight::Absolute(700.0))
                } else if name.eq_ignore_ascii_case("bolder") {
                    Ok(FontWeight::Bolder)
                } else if name.eq_ignore_ascii_case("lighter") {
                    Ok(FontWeight::Lighter)
                } else {
                    Err(())
                }
            }
            Token::Number { value, .. } if (1.0..=1000.0).contains(&value) => {
                Ok(FontWeight::Absolute(value))
            }
            _ => Err(()),
        }
    }
}

impl FontStyle {
    /// `normal | italic | oblique <angle>?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<FontStyle, ()> {
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        if name.eq_ignore_ascii_case("normal") {
            return Ok(FontStyle::Normal);
        }
        if name.eq_ignore_ascii_case("italic") {
            return Ok(FontStyle::Italic);
        }
        if !name.eq_ignore_ascii_case("oblique") {
            return Err(());
        }
        let state = input.state();
        match parse_angle(input) {
            Ok(angle) => Ok(FontStyle::Oblique(angle)),
            Err(()) => {
                input.reset(&state);
                Ok(FontStyle::Oblique(14.0))
            }
        }
    }
}

impl LineHeight {
    /// `normal | <number> | <length-percentage>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<LineHeight, ()> {
        let state = input.state();
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("normal") => Ok(LineHeight::Normal),
            Token::Number { value, .. } if value.is_finite() && value >= 0.0 => {
                Ok(LineHeight::Number(value))
            }
            _ => {
                input.reset(&state);
                Length::parse(input).map(LineHeight::Length)
            }
        }
    }
}

/// `<length-percentage> | <absolute-size> | <relative-size>`.
///
/// The absolute-size keywords scale `medium` by the CSS-standard 1.2 ratio;
/// the relative ones become percentages of the parent's size, so `computed`
/// resolves them with no extra machinery.
pub fn parse_font_size(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
    const ABSOLUTE: &[(&str, f32)] = &[
        ("xx-small", 1.0 / (1.2 * 1.2 * 1.2)),
        ("x-small", 1.0 / (1.2 * 1.2)),
        ("small", 1.0 / 1.2),
        ("medium", 1.0),
        ("large", 1.2),
        ("x-large", 1.2 * 1.2),
        ("xx-large", 1.2 * 1.2 * 1.2),
        ("xxx-large", 1.2 * 1.2 * 1.2 * 1.2),
    ];
    let state = input.state();
    if let Ok(name) = input.expect_ident() {
        let name = name.as_ref().to_string();
        if let Some((_, factor)) = ABSOLUTE
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
        {
            return Ok(Length::px(MEDIUM_FONT_SIZE_PX * factor));
        }
        if name.eq_ignore_ascii_case("larger") {
            return Ok(Length::Percent(1.2));
        }
        if name.eq_ignore_ascii_case("smaller") {
            return Ok(Length::Percent(1.0 / 1.2));
        }
    }
    input.reset(&state);
    Length::parse(input)
}

/// `font-stretch` / `font-width`, always as a CSS percentage where
/// `100` is `normal`.
pub fn parse_stretch(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    const KEYWORDS: &[(&str, f32)] = &[
        ("ultra-condensed", 50.0),
        ("extra-condensed", 62.5),
        ("condensed", 75.0),
        ("semi-condensed", 87.5),
        ("normal", 100.0),
        ("semi-expanded", 112.5),
        ("expanded", 125.0),
        ("extra-expanded", 150.0),
        ("ultra-expanded", 200.0),
    ];
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Ident(ref name) => KEYWORDS
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, percent)| *percent)
            .ok_or(()),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() && unit_value >= 0.0 => {
            Ok(unit_value * 100.0)
        }
        _ => Err(()),
    }
}

fn parse_tag(input: &mut Parser<'_, '_>) -> Result<[u8; 4], ()> {
    let text = input.expect_string().map_err(|_| ())?.as_ref().to_string();
    let bytes = text.as_bytes();
    if bytes.len() != 4 || !text.is_ascii() {
        return Err(());
    }
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// `normal | <feature-tag-value>#`.
pub fn parse_feature_settings(input: &mut Parser<'_, '_>) -> Result<Rc<[FeatureSetting]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident()
        && name.eq_ignore_ascii_case("normal")
    {
        return Ok(Rc::from(Vec::new()));
    }
    input.reset(&state);
    let settings = input
        .parse_comma_separated(|argument| {
            let parsed = (|| {
                let tag = parse_tag(argument)?;
                let state = argument.state();
                let value = match argument.next() {
                    Ok(Token::Number {
                        int_value: Some(number),
                        ..
                    }) => *number,
                    Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("on") => 1,
                    Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("off") => 0,
                    _ => {
                        argument.reset(&state);
                        1
                    }
                };
                Ok::<FeatureSetting, ()>(FeatureSetting { tag, value })
            })();
            match parsed {
                Ok(setting) => Ok(setting),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            }
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    Ok(settings.into())
}

/// `normal | [<opentype-tag> <number>]#`.
pub fn parse_variation_settings(input: &mut Parser<'_, '_>) -> Result<Rc<[VariationSetting]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident()
        && name.eq_ignore_ascii_case("normal")
    {
        return Ok(Rc::from(Vec::new()));
    }
    input.reset(&state);
    let settings = input
        .parse_comma_separated(|argument| {
            let parsed = (|| {
                let tag = parse_tag(argument)?;
                let value = argument.expect_number().map_err(|_| ())?;
                if !value.is_finite() {
                    return Err(());
                }
                Ok::<VariationSetting, ()>(VariationSetting { tag, value })
            })();
            match parsed {
                Ok(setting) => Ok(setting),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            }
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
    Ok(settings.into())
}

#[cfg(test)]
mod tests {
    use super::{
        FontStyle, FontVariantFlags, FontWeight, GenericFamily, LineHeight, parse_family_list,
        parse_feature_settings, parse_font_size, parse_stretch, parse_variant_flags,
        parse_variation_settings,
    };
    use crate::css::value::font::FontFamily;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    #[test]
    fn a_family_list_keeps_order_and_separates_generics_from_names() {
        let families =
            parse_entirely_with("\"Adwaita Sans\", Cantarell, sans-serif", parse_family_list)
                .expect("family list parses");
        assert_eq!(families.len(), 3);
        assert_eq!(families[0], FontFamily::Named("Adwaita Sans".into()));
        assert_eq!(families[1], FontFamily::Named("Cantarell".into()));
        assert_eq!(families[2], FontFamily::Generic(GenericFamily::SansSerif));
        // An unquoted multi-word family joins its idents with spaces.
        let unquoted = parse_entirely_with("Adwaita Sans", parse_family_list).expect("parses");
        assert_eq!(unquoted[0], FontFamily::Named("Adwaita Sans".into()));
    }

    #[test]
    fn font_weight_covers_keywords_relatives_and_numbers() {
        let weight = |text: &str| parse_entirely_with(text, FontWeight::parse).ok();
        assert_eq!(weight("normal"), Some(FontWeight::Absolute(400.0)));
        assert_eq!(weight("BOLD"), Some(FontWeight::Absolute(700.0)));
        assert_eq!(weight("bolder"), Some(FontWeight::Bolder));
        assert_eq!(weight("lighter"), Some(FontWeight::Lighter));
        assert_eq!(weight("350"), Some(FontWeight::Absolute(350.0)));
        assert_eq!(weight("0"), None);
        assert_eq!(weight("1001"), None);
    }

    #[test]
    fn font_style_records_obliques_angle() {
        let style = |text: &str| parse_entirely_with(text, FontStyle::parse).ok();
        assert_eq!(style("normal"), Some(FontStyle::Normal));
        assert_eq!(style("italic"), Some(FontStyle::Italic));
        assert_eq!(style("oblique"), Some(FontStyle::Oblique(14.0)));
        assert_eq!(style("oblique 30deg"), Some(FontStyle::Oblique(30.0)));
    }

    #[test]
    fn font_size_accepts_lengths_percentages_and_the_absolute_keywords() {
        let size = |text: &str| parse_entirely_with(text, parse_font_size).ok();
        assert_eq!(size("14px"), Some(Length::px(14.0)));
        assert_eq!(size("120%"), Some(Length::Percent(1.2)));
        assert_eq!(
            size("1.5em"),
            parse_entirely_with("1.5em", Length::parse).ok()
        );
        // `medium` is the 14px base this engine uses (DEFAULT_FONT_SIZE).
        assert_eq!(size("medium"), Some(Length::px(14.0)));
        assert_eq!(size("large"), Some(Length::px(14.0 * 1.2)));
        assert_eq!(size("small"), Some(Length::px(14.0 / 1.2)));
        assert_eq!(size("larger"), Some(Length::Percent(1.2)));
        assert_eq!(size("smaller"), Some(Length::Percent(1.0 / 1.2)));
    }

    #[test]
    fn font_stretch_maps_keywords_onto_percentages() {
        let stretch = |text: &str| parse_entirely_with(text, parse_stretch).ok();
        assert_eq!(stretch("normal"), Some(100.0));
        assert_eq!(stretch("ultra-condensed"), Some(50.0));
        assert_eq!(stretch("semi-expanded"), Some(112.5));
        assert_eq!(stretch("ultra-expanded"), Some(200.0));
        assert_eq!(stretch("75%"), Some(75.0));
        assert_eq!(stretch("-5%"), None);
    }

    #[test]
    fn line_height_keeps_numbers_and_lengths_apart() {
        let height = |text: &str| parse_entirely_with(text, LineHeight::parse).ok();
        assert_eq!(height("normal"), Some(LineHeight::Normal));
        assert_eq!(height("1.5"), Some(LineHeight::Number(1.5)));
        assert_eq!(height("18px"), Some(LineHeight::Length(Length::px(18.0))));
        assert_eq!(
            height("150%"),
            Some(LineHeight::Length(Length::Percent(1.5)))
        );
    }

    #[test]
    fn feature_and_variation_settings_carry_four_byte_tags() {
        let features = parse_entirely_with("\"liga\" 0, \"tnum\"", parse_feature_settings)
            .expect("feature settings parse");
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].tag, *b"liga");
        assert_eq!(features[0].value, 0);
        assert_eq!(features[1].value, 1);
        let variations = parse_entirely_with("\"wght\" 600", parse_variation_settings)
            .expect("variation settings parse");
        assert_eq!(variations[0].tag, *b"wght");
        assert_eq!(variations[0].value, 600.0);
        // A tag must be exactly four ASCII bytes.
        assert!(parse_entirely_with("\"lig\" 1", parse_feature_settings).is_err());
    }

    #[test]
    fn variant_flags_accumulate_and_reject_values_outside_their_longhand() {
        let caps = FontVariantFlags::SMALL_CAPS | FontVariantFlags::ALL_SMALL_CAPS;
        let parsed = parse_entirely_with("small-caps", |input| parse_variant_flags(input, caps))
            .expect("parses");
        assert_eq!(parsed, FontVariantFlags::SMALL_CAPS);
        assert_eq!(
            parse_entirely_with("normal", |input| parse_variant_flags(input, caps)).ok(),
            Some(FontVariantFlags::empty())
        );
        assert!(parse_entirely_with("ordinal", |input| parse_variant_flags(input, caps)).is_err());
        let numeric = FontVariantFlags::ORDINAL | FontVariantFlags::SLASHED_ZERO;
        assert_eq!(
            parse_entirely_with("ordinal slashed-zero", |input| parse_variant_flags(
                input, numeric
            ))
            .ok(),
            Some(numeric)
        );
    }

    #[test]
    fn font_value_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_family_list);
            let _ = parse_entirely_with(input, FontWeight::parse);
            let _ = parse_entirely_with(input, FontStyle::parse);
            let _ = parse_entirely_with(input, LineHeight::parse);
            let _ = parse_entirely_with(input, parse_font_size);
            let _ = parse_entirely_with(input, parse_stretch);
            let _ = parse_entirely_with(input, parse_feature_settings);
            let _ = parse_entirely_with(input, parse_variation_settings);
            let _ = parse_entirely_with(input, |p| parse_variant_flags(p, FontVariantFlags::all()));
        }
    }
}
