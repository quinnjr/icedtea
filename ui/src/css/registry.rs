//! The property registry: one declarative row per GTK 4.22 CSS property.
//!
//! Nothing outside this module names a property string. `Prop`'s
//! discriminant is the `PROPERTIES` index and, for longhands, the
//! `ComputedStyle` slot, so the enum's declaration order is load-bearing:
//! rows may only be appended to the end of their own group.

use crate::css::value::Value;

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

#[cfg(test)]
mod tests {
    use super::{N_LONGHANDS, N_PROPS, Prop, longhands, lookup};

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
}
