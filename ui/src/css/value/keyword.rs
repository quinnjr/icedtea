//! The one flat keyword enum every discrete property value shares.
//!
//! Sharing one enum (and therefore one `Value` variant) is what lets the
//! registry give every discrete property the same `discrete` interpolator
//! and the same equality semantics without per-property code.

/// Declares `Keyword`, its CSS spellings, `as_str`, `from_str_ascii_ci`
/// and `ALL` from a single table, so the spelling and the variant can never
/// drift apart.
macro_rules! keywords {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum Keyword { $( $variant ),+ }

        impl Keyword {
            /// Every keyword, in declaration order. Test-facing, but cheap
            /// enough (a `&'static` slice) to be public.
            pub const ALL: &'static [Keyword] = &[ $( Keyword::$variant ),+ ];

            /// The keyword's CSS spelling, always ASCII-lowercase.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $( Keyword::$variant => $name ),+ }
            }

            /// ASCII-case-insensitive lookup; `None` for anything else.
            #[must_use]
            pub fn from_str_ascii_ci(s: &str) -> Option<Self> {
                $( if s.eq_ignore_ascii_case($name) { return Some(Keyword::$variant); } )+
                None
            }
        }
    };
}

keywords! {
    // universal
    None => "none", Auto => "auto", Normal => "normal",
    // border / outline style
    Hidden => "hidden", Dotted => "dotted", Dashed => "dashed", Solid => "solid",
    Double => "double", Groove => "groove", Ridge => "ridge",
    Inset => "inset", Outset => "outset",
    // background boxes / repeat / blend
    BorderBox => "border-box", PaddingBox => "padding-box",
    ContentBox => "content-box", TextBox => "text",
    Repeat => "repeat", RepeatX => "repeat-x", RepeatY => "repeat-y",
    Space => "space", Round => "round", NoRepeat => "no-repeat",
    Stretch => "stretch",
    Multiply => "multiply", Screen => "screen", Overlay => "overlay",
    Darken => "darken", Lighten => "lighten", ColorDodge => "color-dodge",
    ColorBurn => "color-burn", HardLight => "hard-light", SoftLight => "soft-light",
    Difference => "difference", Exclusion => "exclusion", Hue => "hue",
    Saturation => "saturation", ColorBlend => "color", Luminosity => "luminosity",
    // fonts
    Italic => "italic", Oblique => "oblique", SmallCaps => "small-caps",
    Bold => "bold", Bolder => "bolder", Lighter => "lighter",
    UltraCondensed => "ultra-condensed", ExtraCondensed => "extra-condensed",
    Condensed => "condensed", SemiCondensed => "semi-condensed",
    SemiExpanded => "semi-expanded", Expanded => "expanded",
    ExtraExpanded => "extra-expanded", UltraExpanded => "ultra-expanded",
    Sub => "sub", Super => "super", AllSmallCaps => "all-small-caps",
    PetiteCaps => "petite-caps", AllPetiteCaps => "all-petite-caps",
    Unicase => "unicase", TitlingCaps => "titling-caps",
    HistoricalForms => "historical-forms", Ruby => "ruby",
    Ordinal => "ordinal", SlashedZero => "slashed-zero",
    XxSmall => "xx-small", XSmall => "x-small", Small => "small",
    Large => "large", XLarge => "x-large", XxLarge => "xx-large",
    XxxLarge => "xxx-large", Larger => "larger", Smaller => "smaller",
    // text
    Capitalize => "capitalize", Uppercase => "uppercase", Lowercase => "lowercase",
    FullWidth => "full-width", FullSizeKana => "full-size-kana",
    Underline => "underline", Overline => "overline",
    LineThrough => "line-through", Blink => "blink", Wavy => "wavy",
    // icons
    Requested => "requested", Regular => "regular", Symbolic => "symbolic",
    Builtin => "builtin",
    // animation
    Forwards => "forwards", Backwards => "backwards", Both => "both",
    Running => "running", Paused => "paused", Reverse => "reverse",
    Alternate => "alternate", AlternateReverse => "alternate-reverse",
    Infinite => "infinite", All => "all",
    // misc
    Cover => "cover", Contain => "contain",
    Left => "left", Right => "right", Top => "top", Bottom => "bottom",
    Center => "center", Closest => "closest", Farthest => "farthest",
    ClosestSide => "closest-side", ClosestCorner => "closest-corner",
    FarthestSide => "farthest-side", FarthestCorner => "farthest-corner",
    Circle => "circle", Ellipse => "ellipse",
    Thin => "thin", Medium => "medium", Thick => "thick",
    Transparent => "transparent", CurrentColor => "currentcolor",
    Invert => "invert", Ltr => "ltr", Rtl => "rtl", Fill => "fill",
}

/// The CSS-wide keywords, valid on every property.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wide {
    /// Take the parent's computed value.
    Inherit,
    /// Take the property's initial value.
    Initial,
    /// `inherit` for inherited properties, `initial` otherwise.
    Unset,
}

impl Wide {
    /// The keyword's CSS spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Wide::Inherit => "inherit",
            Wide::Initial => "initial",
            Wide::Unset => "unset",
        }
    }

    /// ASCII-case-insensitive lookup. `revert`/`revert-layer` are **not**
    /// wide keywords here: GTK has no cascade origins to revert to.
    #[must_use]
    pub fn from_str_ascii_ci(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("inherit") {
            Some(Wide::Inherit)
        } else if s.eq_ignore_ascii_case("initial") {
            Some(Wide::Initial)
        } else if s.eq_ignore_ascii_case("unset") {
            Some(Wide::Unset)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Keyword, Wide};

    #[test]
    fn every_keyword_round_trips_through_its_css_spelling() {
        for keyword in Keyword::ALL {
            assert_eq!(
                Keyword::from_str_ascii_ci(keyword.as_str()),
                Some(*keyword),
                "{} did not round-trip",
                keyword.as_str()
            );
        }
    }

    #[test]
    fn keyword_spellings_are_unique() {
        // A duplicated spelling would make `from_str_ascii_ci` silently
        // shadow one variant, and a registry row would parse to the wrong
        // keyword forever.
        let mut seen: Vec<&'static str> = Keyword::ALL.iter().map(|k| k.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate keyword spelling: {seen:?}");
    }

    #[test]
    fn keyword_lookup_is_ascii_case_insensitive() {
        assert_eq!(Keyword::from_str_ascii_ci("NoNe"), Some(Keyword::None));
        assert_eq!(
            Keyword::from_str_ascii_ci("CURRENTCOLOR"),
            Some(Keyword::CurrentColor)
        );
        assert_eq!(
            Keyword::from_str_ascii_ci("ultra-CONDENSED"),
            Some(Keyword::UltraCondensed)
        );
        assert_eq!(
            Keyword::from_str_ascii_ci("alternate-reverse"),
            Some(Keyword::AlternateReverse)
        );
        assert_eq!(
            Keyword::from_str_ascii_ci("color"),
            Some(Keyword::ColorBlend)
        );
        assert_eq!(Keyword::from_str_ascii_ci("nope"), None);
        assert_eq!(Keyword::from_str_ascii_ci(""), None);
    }

    #[test]
    fn the_wide_keywords_are_recognised_case_insensitively() {
        assert_eq!(Wide::from_str_ascii_ci("inherit"), Some(Wide::Inherit));
        assert_eq!(Wide::from_str_ascii_ci("INITIAL"), Some(Wide::Initial));
        assert_eq!(Wide::from_str_ascii_ci("Unset"), Some(Wide::Unset));
        assert_eq!(Wide::from_str_ascii_ci("revert"), None);
        assert_eq!(Wide::Inherit.as_str(), "inherit");
    }

    #[test]
    fn non_ascii_and_control_bytes_never_panic() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = Keyword::from_str_ascii_ci(input);
            let _ = Wide::from_str_ascii_ci(input);
        }
    }
}
