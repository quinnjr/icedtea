//! Shorthand expansion.
//!
//! The cascade is keyed by *longhand*, so a shorthand has to become
//! longhands before it enters it -- carrying the shorthand's own cascade
//! key, so `border-width: 5px` and a later `border: 1px solid` are ordered
//! by the cascade rather than by which property the consumer happens to
//! read first.
//!
//! Expansion is syntactic: it never resolves a colour (there is no colour
//! table here) and never validates a length. An unusable component simply
//! becomes an unusable longhand value, which the computed-value stage's
//! runner-up fallback then steps over.

use super::value::{comma_groups, component_values};

/// The four sides, in CSS's clockwise order.
const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

/// `border-style` keywords. `none`/`hidden` additionally force a used
/// border width of 0, which the computed stage applies.
const BORDER_STYLES: [&str; 10] = [
    "none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset",
];

/// `<line-width>` keywords. They are bare identifiers, so they must be
/// claimed before the colour test -- which accepts any bare identifier as a
/// named colour -- or `border: thin solid` sets its *colour* to `thin`.
/// `super::computed::parse_border_width` resolves them to px.
const LINE_WIDTHS: [&str; 3] = ["thin", "medium", "thick"];

/// Whether `value` is a `border-style` keyword, and if so whether it
/// suppresses the border entirely (`none`/`hidden` force a used width of 0).
///
/// `None` for anything that is not a border style, so the computed stage can
/// step past an uninterpretable winner like every other property.
#[must_use]
pub fn parse_border_style(value: &str) -> Option<bool> {
    is_keyword(value, &BORDER_STYLES)
        .then(|| value.eq_ignore_ascii_case("none") || value.eq_ignore_ascii_case("hidden"))
}

/// `background` keywords that are positions, repeats, attachments or boxes
/// -- i.e. anything a bare identifier in a `background` value can be
/// *other* than a named colour.
const BACKGROUND_KEYWORDS: [&str; 21] = [
    "none",
    "repeat",
    "repeat-x",
    "repeat-y",
    "no-repeat",
    "space",
    "round",
    "scroll",
    "fixed",
    "local",
    "border-box",
    "padding-box",
    "content-box",
    "cover",
    "contain",
    "center",
    "top",
    "bottom",
    "left",
    "right",
    "auto",
];

/// Colour functions, as opposed to image functions.
const COLOR_FUNCTIONS: [&str; 10] = [
    "rgb", "rgba", "hsl", "hsla", "lab", "lch", "oklab", "oklch", "hwb", "color",
];

fn is_keyword(component: &str, table: &[&str]) -> bool {
    table
        .iter()
        .any(|keyword| component.eq_ignore_ascii_case(keyword))
}

fn function_name(component: &str) -> Option<&str> {
    component
        .split_once('(')
        .filter(|(_, rest)| rest.ends_with(')'))
        .map(|(name, _)| name)
}

/// Whether a component can only be a colour (never an image or a keyword).
fn looks_like_a_colour(component: &str) -> bool {
    if component.starts_with('#') || component.starts_with('@') {
        return true;
    }
    match function_name(component) {
        Some(name) => is_keyword(name, &COLOR_FUNCTIONS),
        // A bare identifier: a named colour, unless it is one of
        // `background`'s own keywords.
        None => {
            !component.is_empty()
                && component
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && !is_keyword(component, &BACKGROUND_KEYWORDS)
        }
    }
}

/// Expand CSS's 1-to-4-value box shorthand over `top right bottom left`.
fn box_sides(prefix: &str, suffix: &str, components: &[String]) -> Vec<(String, String)> {
    let values: [&String; 4] = match components {
        [all] => [all, all, all, all],
        [tb, lr] => [tb, lr, tb, lr],
        [t, lr, b] => [t, lr, b, lr],
        [t, r, b, l] => [t, r, b, l],
        _ => return Vec::new(),
    };
    SIDES
        .iter()
        .zip(values)
        .map(|(side, value)| (format!("{prefix}-{side}{suffix}"), value.clone()))
        .collect()
}

/// Expand `border`/`border-<side>` into width, style and colour longhands.
///
/// Omitted components reset to their CSS initial value (`medium`, `none`,
/// `currentColor`) rather than being left alone, which is what makes
/// `border: none` zero a width an earlier rule set.
fn border_shorthand(sides: &[&str], value: &str) -> Vec<(String, String)> {
    let mut width = "medium".to_string();
    let mut style = "none".to_string();
    let mut color = "currentColor".to_string();
    for component in component_values(value) {
        if is_keyword(&component, &BORDER_STYLES) {
            style = component;
        } else if is_keyword(&component, &LINE_WIDTHS) {
            width = component;
        } else if looks_like_a_colour(&component) {
            color = component;
        } else {
            width = component;
        }
    }
    let mut out = Vec::with_capacity(sides.len() * 3);
    for side in sides {
        out.push((format!("border-{side}-width"), width.clone()));
        out.push((format!("border-{side}-style"), style.clone()));
        out.push((format!("border-{side}-color"), color.clone()));
    }
    out
}

/// Expand `background` into the two longhands this engine paints from.
///
/// Both are always emitted, even when the shorthand names neither: a
/// shorthand resets the longhands it omits, so `background: #112233` must
/// clear an earlier `background-image`.
fn background_shorthand(value: &str) -> Vec<(String, String)> {
    let mut image: Option<String> = None;
    // One colour candidate per layer -- CSS allows one only in the final
    // layer, but GTK's own themes are lax (Adwaita:1359 `junction` puts it
    // first), so a single colour anywhere is accepted. Two or more is
    // genuinely ambiguous, and then only the final layer's counts.
    let mut layer_colors: Vec<Option<String>> = Vec::new();
    for components in comma_groups(value) {
        let mut layer_color = None;
        for component in components {
            if looks_like_a_colour(&component) {
                layer_color = Some(component);
            } else if image.is_none() && function_name(&component).is_some() {
                // The first image found is the one painted on top, and the
                // only one this engine draws.
                image = Some(component);
            }
        }
        layer_colors.push(layer_color);
    }
    let color = layer_colors
        .last()
        .cloned()
        .flatten()
        .or_else(|| {
            let mut declared = layer_colors.iter().flatten();
            let only = declared.next()?;
            declared.next().is_none().then(|| only.clone())
        })
        .unwrap_or_else(|| "transparent".to_string());
    vec![
        ("background-color".to_string(), color),
        (
            "background-image".to_string(),
            image.unwrap_or_else(|| "none".to_string()),
        ),
    ]
}

/// Expand one declaration into the longhands the cascade is keyed by.
///
/// A property that is already a longhand (or that this engine treats as
/// one, such as `border-radius`) passes straight through.
#[must_use]
pub fn expand(name: &str, value: &str) -> Vec<(String, String)> {
    let components = component_values(value);
    match name {
        "padding" | "margin" => {
            let expanded = box_sides(name, "", &components);
            if expanded.is_empty() {
                vec![(name.to_string(), value.to_string())]
            } else {
                expanded
            }
        }
        "border-width" | "border-style" | "border-color" => {
            let suffix = &name["border".len()..];
            let expanded = box_sides("border", suffix, &components);
            if expanded.is_empty() {
                vec![(name.to_string(), value.to_string())]
            } else {
                expanded
            }
        }
        "border" => border_shorthand(&SIDES, value),
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            border_shorthand(&[&name["border-".len()..]], value)
        }
        "background" => background_shorthand(value),
        _ => vec![(name.to_string(), value.to_string())],
    }
}

#[cfg(test)]
mod tests {
    use super::expand;

    fn expanded(name: &str, value: &str) -> Vec<(String, String)> {
        expand(name, value)
    }

    fn get<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn padding_follows_the_one_to_four_value_rule() {
        assert_eq!(
            get(&expanded("padding", "4px"), "padding-left"),
            Some("4px")
        );
        let two = expanded("padding", "4px 9px");
        assert_eq!(get(&two, "padding-top"), Some("4px"));
        assert_eq!(get(&two, "padding-right"), Some("9px"));
        assert_eq!(get(&two, "padding-bottom"), Some("4px"));
        assert_eq!(get(&two, "padding-left"), Some("9px"));
        let three = expanded("padding", "1px 2px 3px");
        assert_eq!(get(&three, "padding-bottom"), Some("3px"));
        assert_eq!(get(&three, "padding-left"), Some("2px"));
    }

    #[test]
    fn border_splits_function_valued_colours_intact() {
        let pairs = expanded("border", "1px solid rgb(0 0 0)");
        assert_eq!(get(&pairs, "border-top-width"), Some("1px"));
        assert_eq!(get(&pairs, "border-top-style"), Some("solid"));
        assert_eq!(get(&pairs, "border-left-color"), Some("rgb(0 0 0)"));
    }

    #[test]
    fn border_resets_the_components_it_omits() {
        let pairs = expanded("border", "none");
        assert_eq!(get(&pairs, "border-top-style"), Some("none"));
        assert_eq!(get(&pairs, "border-top-width"), Some("medium"));
        assert_eq!(get(&pairs, "border-top-color"), Some("currentColor"));
        let adwaita = expanded("border", "1px solid");
        assert_eq!(get(&adwaita, "border-top-width"), Some("1px"));
        assert_eq!(get(&adwaita, "border-top-color"), Some("currentColor"));
    }

    #[test]
    fn a_single_side_border_touches_only_that_side() {
        let pairs = expanded("border-bottom", "2px dashed red");
        assert_eq!(get(&pairs, "border-bottom-width"), Some("2px"));
        assert_eq!(get(&pairs, "border-top-width"), None);
    }

    #[test]
    fn background_separates_colour_from_image() {
        let flat = expanded("background", "#112233");
        assert_eq!(get(&flat, "background-color"), Some("#112233"));
        assert_eq!(get(&flat, "background-image"), Some("none"));

        let image = expanded("background", "image(#e8e6e3)");
        assert_eq!(get(&image, "background-image"), Some("image(#e8e6e3)"));
        assert_eq!(get(&image, "background-color"), Some("transparent"));

        let both = expanded(
            "background",
            "#dfdcd8 linear-gradient(to top, #dad6d2, #e1dedb)",
        );
        assert_eq!(get(&both, "background-color"), Some("#dfdcd8"));
        assert_eq!(
            get(&both, "background-image"),
            Some("linear-gradient(to top, #dad6d2, #e1dedb)")
        );

        let none = expanded("background", "none");
        assert_eq!(get(&none, "background-image"), Some("none"));
        assert_eq!(get(&none, "background-color"), Some("transparent"));

        // `no-repeat` is a keyword, not a named colour.
        let keyworded = expanded("background", "image(#f6f5f4) no-repeat");
        assert_eq!(get(&keyworded, "background-color"), Some("transparent"));
    }

    #[test]
    fn a_longhand_passes_straight_through() {
        assert_eq!(
            expanded("border-radius", "5px"),
            vec![("border-radius".to_string(), "5px".to_string())]
        );
        assert_eq!(
            expanded("min-height", "24px"),
            vec![("min-height".to_string(), "24px".to_string())]
        );
    }

    #[test]
    fn an_unusable_box_shorthand_keeps_its_own_name() {
        // Five components is not a valid `padding`; it must not vanish, so
        // the runner-up fallback can still see it lose.
        assert_eq!(
            expanded("padding", "1px 2px 3px 4px 5px"),
            vec![("padding".to_string(), "1px 2px 3px 4px 5px".to_string())]
        );
    }

    #[test]
    fn line_width_keywords_are_widths_not_colours() {
        // Review round 1: `thin`/`medium`/`thick` are bare identifiers, so
        // the colour test claimed them first -- `border: thin solid` came out
        // as width `medium` and colour `thin`, which then resolved to nothing
        // and let `pick()` resurrect a colour the shorthand should have reset.
        let thin = expanded("border", "thin solid");
        assert_eq!(get(&thin, "border-top-width"), Some("thin"));
        assert_eq!(get(&thin, "border-top-color"), Some("currentColor"));
        let thick = expanded("border", "thick dotted red");
        assert_eq!(get(&thick, "border-top-width"), Some("thick"));
        assert_eq!(get(&thick, "border-top-style"), Some("dotted"));
        assert_eq!(get(&thick, "border-top-color"), Some("red"));
        assert_eq!(
            get(&expanded("border", "medium solid"), "border-top-width"),
            Some("medium")
        );
    }

    #[test]
    fn a_colour_in_a_non_final_background_layer_is_still_found() {
        // Review round 1: Adwaita:1359 puts the colour in the *first* layer.
        // GTK accepts it; CSS does not. One colour anywhere is unambiguous.
        let junction = expanded(
            "background",
            "#cdc7c2, linear-gradient(to bottom, transparent 1px, #cecece 1px),              linear-gradient(to left, transparent 1px, #cecece 1px)",
        );
        assert_eq!(get(&junction, "background-color"), Some("#cdc7c2"));
        assert_eq!(
            get(&junction, "background-image"),
            Some("linear-gradient(to bottom, transparent 1px, #cecece 1px)"),
            "the first layer is the one painted on top"
        );
        // Two candidate colours are ambiguous: only the final layer's counts.
        let ambiguous = expanded("background", "red, blue");
        assert_eq!(get(&ambiguous, "background-color"), Some("blue"));
    }
}
