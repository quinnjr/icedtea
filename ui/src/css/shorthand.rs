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
    let groups = comma_groups(value);
    let mut color = "transparent".to_string();
    let mut image = "none".to_string();
    // Only the final layer may carry the colour; the first layer is the one
    // painted on top, and the only image this engine draws.
    for (index, components) in groups.iter().enumerate() {
        let is_final = index + 1 == groups.len();
        for component in components {
            if is_final && looks_like_a_colour(component) {
                color = component.clone();
            } else if index == 0 && function_name(component).is_some() {
                image = component.clone();
            } else if index == 0 && component.eq_ignore_ascii_case("none") {
                image = "none".to_string();
            }
        }
    }
    vec![
        ("background-color".to_string(), color),
        ("background-image".to_string(), image),
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
}
