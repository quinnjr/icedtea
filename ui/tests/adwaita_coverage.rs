//! THE M2 GATE — every declaration of GTK 4.22's Adwaita, through the registry.
//!
//! Light, dark and high-contrast are all walked: 0 unparseable declarations,
//! 0 unknown properties, 37/37 `@define-color`s resolved. See
//! `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
//! section 7.

/// GTK 4.22 `Default-dark.css`, vendored so this gate is hermetic.
const ADWAITA_DARK: &str = include_str!("../themes/adwaita-dark.css");
/// GTK 4.22 `Default-hc.css`, vendored so this gate is hermetic.
const ADWAITA_HC: &str = include_str!("../themes/adwaita-hc.css");

use cssparser::{Parser, ParserInput};
use icedtea_ui::css::parse::{Declaration, KeyframesRule, StyleRule, Stylesheet, parse_stylesheet};
use icedtea_ui::css::registry::{PropertyKind, lookup};

/// What one sheet's walk found.
#[derive(Default)]
struct Coverage {
    rules: usize,
    declarations: usize,
    /// Property names the registry does not know, in source order.
    unknown: Vec<String>,
    /// `name: value` for declarations the registry knows but cannot parse.
    unparseable: Vec<String>,
}

impl Coverage {
    /// Walk one sheet: its rules, its `@keyframes`, and every `@media` block's
    /// rules and keyframes. `@media` blocks are kept unevaluated by the parser,
    /// so a declaration inside one is still the engine's to read.
    fn walk(css: &str) -> (Coverage, Stylesheet) {
        let sheet = parse_stylesheet(css);
        let mut coverage = Coverage::default();
        coverage.rules(&sheet.rules);
        coverage.keyframes(&sheet.keyframes);
        for block in &sheet.media_blocks {
            coverage.rules(&block.rules);
            coverage.keyframes(&block.keyframes);
        }
        (coverage, sheet)
    }

    fn rules(&mut self, rules: &[StyleRule]) {
        for rule in rules {
            self.rules += 1;
            for declaration in &rule.declarations {
                self.declaration(declaration);
            }
        }
    }

    fn keyframes(&mut self, keyframes: &[KeyframesRule]) {
        for rule in keyframes {
            for (_offsets, declarations) in &rule.frames {
                for declaration in declarations {
                    self.declaration(declaration);
                }
            }
        }
    }

    fn declaration(&mut self, declaration: &Declaration) {
        self.declarations += 1;
        let Some(prop) = lookup(&declaration.name) else {
            self.unknown.push(declaration.name.clone());
            return;
        };
        let mut input = ParserInput::new(&declaration.value);
        let mut parser = Parser::new(&mut input);
        let parsed = match &prop.def().kind {
            PropertyKind::Longhand { parse, .. } => {
                let parse = *parse;
                parse(&mut parser).is_ok()
            }
            PropertyKind::Shorthand { .. } => prop
                .expand_into(&mut parser, &mut |_prop, _value| {})
                .is_ok(),
        };
        // A parser that stopped early left tokens behind: that is a partial
        // read, which is a failure, not a success.
        if !(parsed && parser.is_exhausted()) {
            self.unparseable
                .push(format!("{}: {}", declaration.name, declaration.value));
        }
    }

    /// Fail with the offending declarations named, not with a bare count.
    fn assert_complete(&self, sheet: &str) {
        let mut unknown = self.unknown.clone();
        unknown.sort();
        unknown.dedup();
        assert!(
            self.unknown.is_empty(),
            "{sheet}: {} declarations use {} property names the registry does not know: {unknown:#?}",
            self.unknown.len(),
            unknown.len()
        );
        assert!(
            self.unparseable.is_empty(),
            "{sheet}: {} declarations the registry knows but cannot parse:\n{}",
            self.unparseable.len(),
            self.unparseable.join("\n")
        );
    }
}

#[test]
fn adwaita_light_has_no_unknown_properties_and_no_unparseable_declarations() {
    let (coverage, _sheet) = Coverage::walk(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    assert!(
        coverage.rules > 500,
        "only {} rules walked — the sheet did not parse",
        coverage.rules
    );
    assert!(
        coverage.declarations > 850,
        "only {} declarations walked — the sheet did not parse",
        coverage.declarations
    );
    coverage.assert_complete("Adwaita light");
}

#[test]
fn the_instrument_reports_what_it_cannot_read() {
    // Negative control: without this, a walk that silently accepted everything
    // would still pass the gate.
    let (coverage, _sheet) = Coverage::walk(
        "button { -gtk-not-a-property: 1px; color: nosuchfunction(1); \
         border: 1px solid #cdc7c2; }",
    );
    assert_eq!(coverage.rules, 1);
    assert_eq!(coverage.declarations, 3);
    assert_eq!(coverage.unknown, vec!["-gtk-not-a-property".to_string()]);
    assert_eq!(
        coverage.unparseable,
        vec!["color: nosuchfunction(1)".to_string()],
        "an uninterpretable value must be reported, not counted as read"
    );
}

#[test]
fn a_trailing_token_is_a_failed_read_not_a_partial_one() {
    // `parse_entirely`'s rule, asserted directly: a parser that stops early has
    // not read the declaration.
    let (coverage, _sheet) = Coverage::walk("button { opacity: 0.5 0.5; }");
    assert_eq!(
        coverage.unparseable,
        vec!["opacity: 0.5 0.5".to_string()],
        "a value with a trailing token was accepted"
    );
}

#[test]
fn keyframe_and_media_declarations_are_walked_too() {
    let (coverage, sheet) = Coverage::walk(
        "@keyframes spin { to { transform: rotate(1turn); } } \
         @media (prefers-color-scheme: dark) { button { color: white; } }",
    );
    assert_eq!(sheet.keyframes.len(), 1, "the @keyframes rule was dropped");
    assert_eq!(sheet.media_blocks.len(), 1, "the @media block was dropped");
    assert_eq!(
        coverage.declarations, 2,
        "a declaration inside @keyframes or @media was not walked"
    );
    coverage.assert_complete("synthetic");
}

#[test]
fn the_vendored_sheets_are_the_extracted_gtk4_themes() {
    assert_eq!(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT.lines().count(),
        1941,
        "vendored Adwaita light is not GTK 4.22's 1,941-line Default-light.css"
    );
    assert_eq!(
        ADWAITA_DARK.lines().count(),
        1929,
        "vendored Adwaita dark is not GTK 4.22's 1,929-line Default-dark.css"
    );
    assert_eq!(
        ADWAITA_HC.lines().count(),
        1944,
        "vendored Adwaita high-contrast is not GTK 4.22's 1,944-line Default-hc.css"
    );
    for (name, sheet) in [
        ("light", icedtea_ui::BUNDLED_ADWAITA_LIGHT),
        ("dark", ADWAITA_DARK),
        ("hc", ADWAITA_HC),
    ] {
        assert_eq!(
            sheet.matches("@define-color").count(),
            37,
            "vendored Adwaita {name} does not carry GTK 4.22's 37 @define-color declarations"
        );
    }
}

#[test]
fn the_three_sheets_are_three_different_themes() {
    assert_ne!(icedtea_ui::BUNDLED_ADWAITA_LIGHT, ADWAITA_DARK);
    assert_ne!(icedtea_ui::BUNDLED_ADWAITA_LIGHT, ADWAITA_HC);
    assert_ne!(ADWAITA_DARK, ADWAITA_HC);
    // The colour that separates them, declared in each sheet's own words.
    assert!(icedtea_ui::BUNDLED_ADWAITA_LIGHT.contains("@define-color theme_bg_color #f6f5f4"));
    assert!(ADWAITA_DARK.contains("@define-color theme_bg_color #353535"));
    assert!(ADWAITA_HC.contains("@define-color theme_bg_color #fdfdfc"));
}

use icedtea_ui::css::value::color::{ColorCtx, Rgba, build_color_table};

/// Resolve every `@define-color` in a sheet, returning the names that did not
/// resolve. Definitions inside `@media` blocks count too.
fn unresolved_colors(sheet: &Stylesheet) -> Vec<String> {
    let mut definitions = sheet.color_definitions.clone();
    for block in &sheet.media_blocks {
        definitions.extend(block.color_definitions.iter().cloned());
    }
    let pairs: Vec<(String, String)> = definitions
        .iter()
        .map(|definition| (definition.name.clone(), definition.value.clone()))
        .collect();
    let table = build_color_table(&pairs);
    let ctx = ColorCtx {
        table: &table,
        // `@define-color` bodies are resolved outside any element, so
        // `currentColor` has nothing to be but the initial colour.
        current: Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
        depth: 0,
    };
    let mut unresolved = Vec::new();
    for definition in &definitions {
        let name = &definition.name;
        match table.get(name) {
            Some(value) => {
                if value.resolve(&ctx).is_none() {
                    unresolved.push(name.clone());
                }
            }
            None => unresolved.push(name.clone()),
        }
    }
    unresolved
}

/// One sheet, walked end to end: the whole gate for that sheet.
///
/// `rules` and `declarations` are the *exact* counts this sheet must walk.
/// They used to be floors of 500 and 850 against real counts of ~900 and
/// ~2330 — a 2.6x margin, wide enough for a parser regression that silently
/// dropped 60% of Adwaita's declarations to still report zero unknown and
/// zero unparseable properties and pass the gate. `parse_stylesheet` skips
/// an invalid rule with a `tracing::debug` and drops a nested `@media` block
/// whole, neither of which shows up in the unknown/unparseable lists, so the
/// count is the only thing that catches them. Pinned exactly, as
/// `the_vendored_sheets_are_the_extracted_gtk4_themes` pins line counts.
fn assert_sheet_is_fully_covered(name: &str, css: &str, rules: usize, declarations: usize) {
    let (coverage, sheet) = Coverage::walk(css);
    assert_eq!(
        (coverage.rules, coverage.declarations),
        (rules, declarations),
        "{name}: walked {} rules / {} declarations, expected {rules} / {declarations} — \
         either the vendored sheet changed or the parser started dropping rules",
        coverage.rules,
        coverage.declarations
    );
    coverage.assert_complete(name);

    let definitions = sheet.color_definitions.len();
    assert_eq!(
        definitions, 37,
        "{name}: {definitions} @define-color declarations reached the sheet, GTK 4.22 has 37"
    );
    let unresolved = unresolved_colors(&sheet);
    assert!(
        unresolved.is_empty(),
        // `unresolved` gathers every `@media` block's definitions too, so
        // its length can exceed 37; a plain `37 - len` panicked with a
        // subtract-overflow on the failure path instead of naming the
        // colours that did not resolve.
        "{name}: {}/37 @define-colors resolved; these did not: {unresolved:#?}",
        37_usize.saturating_sub(unresolved.len())
    );
}

#[test]
fn adwaita_light_resolves_all_37_define_colors() {
    // M1 pinned 29/37: the eight relative-colour definitions
    // (`hsl(from … calc(s * 1.8) …)`, `rgb(from black r g b / calc(alpha * .35))`)
    // were recorded unresolved rather than fabricated. Resolving all 37 is an
    // M2 deliverable (spec Decision 6 / contract section 2.4), not a regression.
    let sheet = parse_stylesheet(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    assert_eq!(sheet.color_definitions.len(), 37);
    assert!(
        sheet.color_definitions.iter().any(|definition| {
            definition.name == "wm_title" && definition.value.contains("hsl(from")
        }),
        "the relative-colour definitions are not in the sheet this test claims to cover"
    );
    assert!(unresolved_colors(&sheet).is_empty());
}

#[test]
fn a_definition_may_reference_a_name_defined_later() {
    // Lazy, order-independent resolution — M1 required earlier-only.
    let sheet = parse_stylesheet(
        "@define-color a alpha(@b, 0.5); @define-color b #3584e4; button { color: @a; }",
    );
    assert!(
        unresolved_colors(&sheet).is_empty(),
        "a forward reference did not resolve"
    );
}

#[test]
fn a_colour_cycle_is_broken_not_hung() {
    let sheet = parse_stylesheet("@define-color a shade(@b, 1.1); @define-color b shade(@a, 1.1);");
    // The point is that this returns at all; both names are legitimately
    // unresolvable, and neither may hang or overflow the stack.
    assert_eq!(unresolved_colors(&sheet).len(), 2);
}

#[test]
fn the_m2_gate_adwaita_light_dark_and_high_contrast() {
    assert_sheet_is_fully_covered(
        "Adwaita light",
        icedtea_ui::BUNDLED_ADWAITA_LIGHT,
        900,
        2327,
    );
    assert_sheet_is_fully_covered("Adwaita dark", ADWAITA_DARK, 894, 2316);
    assert_sheet_is_fully_covered("Adwaita high-contrast", ADWAITA_HC, 902, 2333);
}
