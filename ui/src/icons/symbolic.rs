//! GTK's four-slot symbolic recolouring.
//!
//! A symbolic icon is an SVG authored with exactly four logical fills: three
//! carrying the CSS classes `success`/`warning`/`error`, and everything else
//! taking the icon's foreground. GTK recolours them by reassigning the
//! parsed document's paints —
//! `gtk_icon_paintable_snapshot_symbolic(.., colors[4])` — not by patching
//! the SVG text, and this does the same.
//!
//! Three things have to happen together or the recolour does not survive to
//! the canvas, because `render_svg_in_container` re-clones the dom and
//! re-applies its stylesheet, and `apply_stylesheet` applies each node's
//! inline `style=` attribute *after* every rule:
//!
//! 1. the document's own `fill` declarations are removed from its stylesheet,
//! 2. `fill`/`fill-opacity` are removed from every inline `style=`, and
//! 3. the palette's rules are both applied *and* left in the document's
//!    stylesheet, so the re-application inside the renderer lands on them.
//!
//! All three operate on a clone: the cached parse is never mutated.

use skia_rs_safe::svg::css::StyleDeclarations;
use skia_rs_safe::svg::{
    CssRule, CssSelector, Stylesheet, SvgDom, SvgNode, apply_stylesheet, parse_inline_style,
};

use super::theme::{IconFile, Palette};
use crate::css::value::Rgba;

/// The deepest SVG tree this will recolour.
///
/// `apply_stylesheet` recurses, so a document deeper than this is refused
/// rather than recoloured — a 100 000-deep `<g>` chain in a file called
/// `foo-symbolic.svg` must not be able to overflow the stack. Real icons are
/// under twenty deep.
pub(crate) const MAX_SVG_DEPTH: usize = 128;

/// The most nodes this will walk.
pub(crate) const MAX_SVG_NODES: usize = 100_000;

/// `#rrggbb` for a colour, which is what the SVG paint parser accepts.
fn hex(color: Rgba) -> String {
    let packed = color.to_color32().0;
    format!(
        "#{:02x}{:02x}{:02x}",
        (packed >> 16) & 0xFF,
        (packed >> 8) & 0xFF,
        packed & 0xFF
    )
}

/// One `<selector> { fill: …; fill-opacity: … }` rule.
///
/// The alpha travels as `fill-opacity` rather than in the colour because
/// `fill` is parsed as an opaque paint; `apply_style_property` treats
/// `fill-opacity` as the independent inherited property SVG says it is and
/// multiplies it in at render time.
fn rule(selector: &str, color: Rgba) -> CssRule {
    let mut declarations = StyleDeclarations::new();
    declarations.insert("fill".to_owned(), hex(color));
    declarations.insert(
        "fill-opacity".to_owned(),
        format!("{}", color.a.clamp(0.0, 1.0)),
    );
    CssRule {
        selector: CssSelector::parse(selector),
        declarations,
    }
}

/// The four rules one palette produces.
///
/// The foreground is a universal selector so it reaches every shape with no
/// class of its own; its `(0, 0, 0)` specificity is below the three class
/// rules' `(0, 1, 0)`, and `apply_stylesheet` sorts ascending by specificity,
/// so the class rules land last and win.
fn palette_rules(palette: &Palette) -> Vec<CssRule> {
    vec![
        rule("*", palette.foreground),
        rule(".success", palette.success),
        rule(".warning", palette.warning),
        rule(".error", palette.error),
    ]
}

/// Remove every `fill`/`fill-opacity` declaration from a stylesheet's rules.
///
/// `StyleDeclarations` has no removal API, so each rule's declarations are
/// rebuilt without those two properties.
fn strip_fill_from_stylesheet(sheet: &mut Stylesheet) {
    for rule in &mut sheet.rules {
        let mut kept = StyleDeclarations::new();
        for (property, value) in rule.declarations.iter() {
            if property != "fill" && property != "fill-opacity" {
                kept.insert(property.clone(), value.clone());
            }
        }
        rule.declarations = kept;
    }
}

/// The tree's depth and node count, walked iteratively.
///
/// `None` when either bound is exceeded.
///
/// Visible to the module because *every* parsed document is measured at
/// parse time (`super::svg_parse_logged`), not only the symbolic ones: the
/// renderer recurses over the tree whether or not it was recoloured.
pub(crate) fn measure(root: &SvgNode) -> Option<()> {
    let mut stack: Vec<(&SvgNode, usize)> = vec![(root, 1)];
    let mut nodes = 0usize;
    while let Some((node, depth)) = stack.pop() {
        nodes += 1;
        if depth > MAX_SVG_DEPTH || nodes > MAX_SVG_NODES {
            return None;
        }
        for child in &node.children {
            stack.push((child, depth + 1));
        }
    }
    Some(())
}

/// Remove `fill`/`fill-opacity` from every inline `style=` attribute.
///
/// Walked with an explicit stack: the depth is already bounded by
/// [`measure`], and an explicit stack keeps this function itself off the
/// recursion budget `apply_stylesheet` is about to spend.
fn strip_inline_fill(root: &mut SvgNode) {
    let mut stack: Vec<&mut SvgNode> = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(style) = node.attributes.get("style") {
            let declarations = parse_inline_style(style);
            let kept: Vec<String> = declarations
                .iter()
                .filter(|(property, _)| *property != "fill" && *property != "fill-opacity")
                .map(|(property, value)| format!("{property}:{value}"))
                .collect();
            if kept.is_empty() {
                node.attributes.remove("style");
            } else {
                node.attributes.insert("style".to_owned(), kept.join(";"));
            }
        }
        for child in &mut node.children {
            stack.push(child);
        }
    }
}

/// Recolour a symbolic icon's parsed document for one palette.
///
/// Returns a clone: the caller's `SvgDom` is a cached parse shared by every
/// palette, and mutating it would recolour every other node that shares it.
/// A document that exceeds `MAX_SVG_DEPTH`/`MAX_SVG_NODES` comes back
/// cloned and unrecoloured — a bounded wrong colour, never an unbounded
/// stack.
#[must_use]
pub fn recolour(dom: &SvgDom, palette: &Palette) -> SvgDom {
    let mut working = dom.clone();
    if measure(&working.root).is_none() {
        tracing::debug!("symbolic SVG exceeds the depth or node budget; drawn without recolouring");
        return working;
    }
    strip_fill_from_stylesheet(&mut working.stylesheet);
    strip_inline_fill(&mut working.root);
    let rules = palette_rules(palette);
    let sheet = Stylesheet {
        rules: rules.clone(),
    };
    apply_stylesheet(&mut working, &sheet);
    working.stylesheet.rules.extend(rules);
    working
}

/// `true` for a `-symbolic.svg` name or a file under a `symbolic/` subdir.
#[must_use]
pub fn is_symbolic(file: &IconFile) -> bool {
    file.symbolic || super::lookup::path_is_symbolic(&file.path)
}

#[cfg(test)]
mod tests {
    use super::{MAX_SVG_DEPTH, is_symbolic, recolour};
    use crate::css::value::Rgba;
    use crate::icons::test_support::roots;
    use crate::icons::{DirKind, IconFile, IconFormat, Palette};
    use skia_rs_safe::svg::{SvgDom, SvgNode, SvgNodeKind, SvgPaint, parse_svg};

    fn rgba(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    /// A palette whose four slots are all *different from the defaults*.
    ///
    /// Deliberately not `Palette::for_color`: these are the GNOME 42
    /// success/warning/error hexes, and the vendored Adwaita sheet this crate
    /// ships uses different ones (`theme::DEFAULT_SUCCESS` and friends; contract §10
    /// P7-D49). A test that reads a slot back therefore proves the slot was
    /// actually used, rather than that a default happened to match.
    fn test_palette() -> Palette {
        Palette {
            foreground: rgba(0x35, 0x84, 0xe4),
            success: rgba(0x26, 0xa2, 0x69),
            warning: rgba(0xcd, 0x93, 0x09),
            error: rgba(0xe0, 0x1b, 0x24),
        }
    }

    fn fixture_dom() -> SvgDom {
        let path = roots()[0].join("MiniTheme/symbolic/actions/document-open-symbolic.svg");
        let text = std::fs::read_to_string(path).expect("fixture readable");
        parse_svg(&text).expect("fixture parses")
    }

    // `SvgPaint` (skia-rs-svg 0.4.0) derives `Debug`/`Clone` but not
    // `PartialEq`, so fills are compared through their `Debug` rendering
    // rather than `assert_eq!` on the enum directly (reconciliation: the
    // plan's test compared `Vec<Option<SvgPaint>>` with `==`).
    fn rect_fills(dom: &SvgDom) -> Vec<Option<SvgPaint>> {
        let mut out = Vec::new();
        let mut stack: Vec<&SvgNode> = vec![&dom.root];
        while let Some(node) = stack.pop() {
            if matches!(node.kind, SvgNodeKind::Rect(_)) {
                out.push(node.fill.clone());
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
        out
    }

    fn color_of(paint: &Option<SvgPaint>) -> u32 {
        match paint {
            Some(SvgPaint::Color(color)) => color.0,
            other => panic!("expected a solid colour fill, got {other:?}"),
        }
    }

    // The whole point: an unclassed shape becomes the foreground and a
    // `.success` shape becomes the success slot -- even though the fixture
    // pins both to #bebebe, one through an inline style and one through a
    // fill attribute plus an embedded stylesheet.
    // Mutation check: drop the universal catch-all rule and the first fill
    // stays #bebebe.
    #[test]
    fn unclassed_shapes_take_the_foreground_and_classed_ones_their_slot() {
        let dom = fixture_dom();
        let recoloured = recolour(&dom, &test_palette());
        let fills = rect_fills(&recoloured);
        assert_eq!(fills.len(), 2);
        assert_eq!(color_of(&fills[0]), 0xFF35_84E4);
        assert_eq!(color_of(&fills[1]), 0xFF26_A269);
    }

    // The cached parse must survive: `IconTheme::render` keeps one `SvgDom`
    // per file and recolours a clone per palette.
    // Mutation check: recolour in place (take `&mut SvgDom`) and the input's
    // fills change with the output's.
    #[test]
    fn recolour_never_touches_its_input() {
        let dom = fixture_dom();
        let before = format!("{:?}", rect_fills(&dom));
        let _ = recolour(&dom, &test_palette());
        assert_eq!(format!("{:?}", rect_fills(&dom)), before);
    }

    // `render_svg_in_container` re-applies the document's own stylesheet on
    // its own clone, so the palette rules have to be *in* that stylesheet or
    // the recolour is undone at draw time (deviation 7).
    // Mutation check: stop extending `working.stylesheet.rules` and this
    // fails -- and so does the recolour pixel test in Task 10, which is the
    // one that actually matters.
    #[test]
    fn the_palette_rules_are_left_in_the_documents_own_stylesheet() {
        let recoloured = recolour(&fixture_dom(), &test_palette());
        let with_fill = recoloured
            .stylesheet
            .rules
            .iter()
            .filter(|rule| rule.declarations.get("fill").is_some())
            .count();
        assert_eq!(
            with_fill, 4,
            "only the four palette rules may declare a fill; the document's own \
             fill declarations must have been stripped"
        );
    }

    // And the inline `style="fill:…"` must be gone, because
    // `apply_stylesheet` applies inline styles *after* every rule.
    // Mutation check: keep the inline declaration and the first rect comes
    // back #bebebe.
    #[test]
    fn inline_fill_declarations_are_stripped_from_the_clone() {
        let recoloured = recolour(&fixture_dom(), &test_palette());
        let mut stack: Vec<&SvgNode> = vec![&recoloured.root];
        while let Some(node) = stack.pop() {
            if let Some(style) = node.attributes.get("style") {
                assert!(
                    !style.contains("fill"),
                    "inline style survived recolour: {style:?}"
                );
            }
            for child in &node.children {
                stack.push(child);
            }
        }
    }

    // `is_symbolic` reads the file, not the request.
    // Mutation check: check only the filename and the `symbolic/` directory
    // case fails -- which is how Adwaita's own symbolic icons are laid out
    // when a theme drops the suffix.
    #[test]
    fn a_file_is_symbolic_by_its_name_or_its_directory() {
        let make = |path: &str| IconFile {
            path: std::path::PathBuf::from(path),
            format: IconFormat::Svg,
            nominal_size: 16,
            scale: 1,
            symbolic: false,
            kind: DirKind::Scalable { min: 8, max: 512 },
        };
        assert!(is_symbolic(&make("/t/scalable/a-symbolic.svg")));
        assert!(is_symbolic(&make("/t/symbolic/actions/a.svg")));
        assert!(!is_symbolic(&make("/t/scalable/actions/a.svg")));
        assert!(!is_symbolic(&make("/t/scalable/actions/symbolics.svg")));
    }

    // An SVG is untrusted input, and a pathological one must cost a bounded
    // amount of work rather than the stack.
    // Mutation check: recurse instead of walking with an explicit stack, or
    // drop the depth guard, and this overflows the stack.
    #[test]
    fn a_pathological_dom_is_refused_rather_than_recoloured() {
        let mut root = SvgNode::new(SvgNodeKind::Svg);
        let mut deep = SvgNode::new(SvgNodeKind::Group);
        for _ in 0..(MAX_SVG_DEPTH * 4) {
            let mut parent = SvgNode::new(SvgNodeKind::Group);
            parent.add_child(deep);
            deep = parent;
        }
        root.add_child(deep);
        let dom = SvgDom {
            root,
            ..SvgDom::new()
        };
        // Returns a clone, unrecoloured, rather than panicking or recursing.
        let out = recolour(&dom, &test_palette());
        assert!(out.stylesheet.rules.is_empty());

        // The trivial cases must not panic either.
        let _ = recolour(&SvgDom::new(), &test_palette());
    }
}
