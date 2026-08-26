//! `cssparser` glue: CSS text in, `(selector text, declarations)` rules and
//! `@define-color` pairs out.
//!
//! Deliberately value-agnostic *in meaning*, but never in form: declaration
//! values are the **serialization of their token stream**
//! (`super::tokens`), so comments are gone and whitespace is normalized
//! before any consumer sees them. Interpretation still happens later
//! (`super::colors`, `super::computed`), which keeps every GTK-specific
//! value form (`image()`, `-gtk-*`, relative color syntax) representable
//! even when this milestone cannot yet interpret it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
    parse_important,
};

use super::tokens::serialize_remaining;

/// How deep `@import` chains are followed before the engine gives up.
pub const MAX_IMPORT_DEPTH: usize = 8;

/// One `name: value` pair from a declaration block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// Property name, ASCII-lowercased.
    pub name: String,
    /// The value, serialized from its token stream: comments removed,
    /// whitespace collapsed, `!important` stripped.
    pub value: String,
    /// Whether the declaration carried `!important`.
    pub important: bool,
}

/// One qualified rule: its prelude as source text plus its declarations.
#[derive(Debug, Clone)]
pub struct StyleRule {
    /// The prelude verbatim, e.g. `"window > button, headerbar button:hover"`.
    pub selector_text: String,
    /// The rule's declarations, in source order.
    pub declarations: Vec<Declaration>,
    /// 0-based index of this rule in the sheet; the cascade's final tiebreak.
    pub source_order: usize,
}

/// A parsed stylesheet.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    /// Qualified rules, in source order (imports spliced in at their site).
    pub rules: Vec<StyleRule>,
    /// `(name, value)` for every `@define-color`, in source order.
    pub color_definitions: Vec<(String, String)>,
}

/// Parses a single declaration block.
///
/// `RuleBodyItemParser` demands one item type for declarations, nested
/// qualified rules and nested at-rules alike, so the item is
/// `Option<Declaration>`: `None` is a nested rule, which is consumed and
/// discarded so the declarations *after* it are still seen (GTK has no
/// nesting, but a sheet that uses it must not lose its remaining
/// properties).
struct DeclarationBlockParser;

impl<'i> DeclarationParser<'i> for DeclarationBlockParser {
    type Declaration = Option<Declaration>;
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _start: &ParserState,
    ) -> Result<Option<Declaration>, ParseError<'i, ()>> {
        let mut value = String::new();
        let mut important = false;
        loop {
            if input.is_exhausted() {
                break;
            }
            // `!important` is detected with cssparser's own helper, which
            // skips whitespace and comments -- so `red !important /* c */`
            // still carries the flag.
            if input.try_parse(parse_important).is_ok() {
                important = true;
                while input.next().is_ok() {}
                break;
            }
            let before = input.position();
            super::tokens::write_one_component(input, &mut value);
            if input.position() == before {
                break;
            }
        }
        while value.ends_with(' ') {
            value.pop();
        }
        Ok(Some(Declaration {
            name: name.as_ref().to_ascii_lowercase(),
            value,
            important,
        }))
    }
}

impl<'i> AtRuleParser<'i> for DeclarationBlockParser {
    type Prelude = ();
    type AtRule = Option<Declaration>;
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for DeclarationBlockParser {
    type Prelude = ();
    type QualifiedRule = Option<Declaration>;
    type Error = ();

    fn parse_prelude<'t>(&mut self, input: &mut Parser<'i, 't>) -> Result<(), ParseError<'i, ()>> {
        while input.next().is_ok() {}
        Ok(())
    }

    fn parse_block<'t>(
        &mut self,
        _prelude: (),
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Option<Declaration>, ParseError<'i, ()>> {
        while input.next().is_ok() {}
        Ok(None)
    }
}

impl<'i> RuleBodyItemParser<'i, Option<Declaration>, ()> for DeclarationBlockParser {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        true
    }
}

/// What a top-level item in a sheet turned out to be.
enum SheetItem {
    /// A qualified rule.
    Rule(StyleRule),
    /// `@define-color <name> <value>`.
    Color(String, String),
    /// `@import <url>`.
    Import(String),
    /// Something parsed but carrying nothing this engine keeps.
    Ignored,
}

/// Top-level rule-list parser.
struct SheetParser;

impl<'i> QualifiedRuleParser<'i> for SheetParser {
    type Prelude = String;
    type QualifiedRule = SheetItem;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<String, ParseError<'i, ()>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(input.slice_from(start).trim().to_string())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: String,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<SheetItem, ParseError<'i, ()>> {
        let mut block = DeclarationBlockParser;
        // A malformed declaration must not discard the rest of the block --
        // GTK themes routinely carry properties this milestone's tokenizer
        // path cannot interpret, so errors are simply dropped.
        let declarations: Vec<Declaration> = RuleBodyParser::<_, _, ()>::new(input, &mut block)
            .flatten()
            .flatten()
            .collect();
        Ok(SheetItem::Rule(StyleRule {
            selector_text: prelude,
            declarations,
            // Filled in by `parse_into`, which knows the index.
            source_order: 0,
        }))
    }
}

/// The prelude of an at-rule this engine understands.
enum AtPrelude {
    Color(String, String),
    Import(String),
}

/// Read an `@import` prelude's URL: `'x.css'`, `"x.css"` or `url(...)`.
fn parse_import_url<'i>(input: &mut Parser<'i, '_>) -> Result<String, ParseError<'i, ()>> {
    let token = input.next()?.clone();
    let url = match token {
        Token::QuotedString(value) | Token::UnquotedUrl(value) => value.as_ref().to_string(),
        Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
            input.parse_nested_block(|inner| {
                Ok::<String, ParseError<'i, ()>>(inner.expect_string()?.as_ref().to_string())
            })?
        }
        _ => return Err(input.new_custom_error(())),
    };
    // Media queries after the URL are not modelled; drop them.
    while input.next().is_ok() {}
    Ok(url)
}

impl<'i> AtRuleParser<'i> for SheetParser {
    type Prelude = AtPrelude;
    type AtRule = SheetItem;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<AtPrelude, ParseError<'i, ()>> {
        if name.eq_ignore_ascii_case("import") {
            return parse_import_url(input).map(AtPrelude::Import);
        }
        if !name.eq_ignore_ascii_case("define-color") {
            return Err(input.new_custom_error(()));
        }
        let color_name = input.expect_ident()?.as_ref().to_string();
        let value = serialize_remaining(input);
        if value.is_empty() {
            return Err(input.new_custom_error(()));
        }
        Ok(AtPrelude::Color(color_name, value))
    }

    /// The at-rules this engine keeps carry no block, so a block means the
    /// rule was malformed: the definition is *not* recorded. (Recording it
    /// from `parse_prelude` would let an invalid at-rule mutate the colour
    /// table.)
    fn parse_block<'t>(
        &mut self,
        _prelude: AtPrelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<SheetItem, ParseError<'i, ()>> {
        while input.next().is_ok() {}
        Ok(SheetItem::Ignored)
    }

    fn rule_without_block(
        &mut self,
        prelude: AtPrelude,
        _start: &ParserState,
    ) -> Result<SheetItem, ()> {
        Ok(match prelude {
            AtPrelude::Color(name, value) => SheetItem::Color(name, value),
            AtPrelude::Import(url) => SheetItem::Import(url),
        })
    }
}

/// Parse `css` into rules and `@define-color` pairs.
///
/// `@import` is only resolvable relative to a base directory, so this
/// base-less wrapper logs and skips every import. Callers that read a sheet
/// off disk should use [`parse_stylesheet_with_base`].
///
/// Invalid rules are skipped rather than aborting the sheet, matching CSS's
/// own error-recovery rules and GTK's tolerance of unknown syntax.
#[must_use]
pub fn parse_stylesheet(css: &str) -> Stylesheet {
    parse_stylesheet_with_base(css, None)
}

/// Parse `css`, resolving `@import` relative to `base_dir`.
///
/// Imports are spliced in at their site (so source order, and therefore the
/// cascade, is the order a browser would see), followed recursively to a
/// depth of [`MAX_IMPORT_DEPTH`], and guarded by a visited set so a cycle
/// terminates. `resource://` URLs -- GTK's own bundled resources -- are
/// skipped with a debug log, as is any import when `base_dir` is `None`.
#[must_use]
pub fn parse_stylesheet_with_base(css: &str, base_dir: Option<&Path>) -> Stylesheet {
    let mut sheet = Stylesheet::default();
    let mut visited = HashSet::new();
    parse_into(css, base_dir, 0, &mut visited, &mut sheet);
    sheet
}

fn parse_into(
    css: &str,
    base_dir: Option<&Path>,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Stylesheet,
) {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut sheet_parser = SheetParser;
    for item in StyleSheetParser::new(&mut parser, &mut sheet_parser) {
        match item {
            Ok(SheetItem::Rule(mut rule)) => {
                rule.source_order = out.rules.len();
                out.rules.push(rule);
            }
            Ok(SheetItem::Color(name, value)) => out.color_definitions.push((name, value)),
            Ok(SheetItem::Import(url)) => resolve_import(&url, base_dir, depth, visited, out),
            Ok(SheetItem::Ignored) => {}
            Err((err, slice)) => {
                tracing::debug!(?err, rule = %slice.chars().take(80).collect::<String>(), "skipping invalid CSS rule");
            }
        }
    }
}

fn resolve_import(
    url: &str,
    base_dir: Option<&Path>,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Stylesheet,
) {
    if depth >= MAX_IMPORT_DEPTH {
        tracing::debug!(%url, depth, "@import nesting limit reached; skipping");
        return;
    }
    if url.contains("://") {
        tracing::debug!(%url, "non-file @import URL (GTK resource?); skipping");
        return;
    }
    let Some(base_dir) = base_dir else {
        tracing::debug!(%url, "@import with no base directory to resolve against; skipping");
        return;
    };
    let path = base_dir.join(url);
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if !visited.insert(canonical) {
        tracing::debug!(path = %path.display(), "@import cycle; skipping");
        return;
    }
    let css = match std::fs::read_to_string(&path) {
        Ok(css) => css,
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "cannot read @import target; skipping");
            return;
        }
    };
    let nested_base = path.parent().map(Path::to_path_buf);
    parse_into(&css, nested_base.as_deref(), depth + 1, visited, out);
}

#[cfg(test)]
mod tests {
    use super::{Declaration, parse_stylesheet, parse_stylesheet_with_base};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh scratch directory for the `@import` tests.
    ///
    /// Unique per call -- pid plus a monotonic counter -- so two tests (or
    /// two concurrent `cargo test` runs) never collide. Nothing is wiped up
    /// front: a name that has never been used cannot hold anything, and
    /// wiping would have deleted a live directory on a name collision.
    fn tempdir(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("icedtea-ui-{tag}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn decl<'a>(decls: &'a [Declaration], name: &str) -> &'a str {
        decls
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("no `{name}` declaration in {decls:?}"))
            .value
            .as_str()
    }

    #[test]
    fn parses_selector_text_and_declarations() {
        let sheet = parse_stylesheet(
            "window > button { color: #2e3436; padding: 4px 9px }\n\
             button:hover { border-color: red !important }",
        );
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.rules[0].selector_text, "window > button");
        assert_eq!(sheet.rules[0].source_order, 0);
        assert_eq!(decl(&sheet.rules[0].declarations, "color"), "#2e3436");
        assert_eq!(decl(&sheet.rules[0].declarations, "padding"), "4px 9px");
        assert_eq!(sheet.rules[1].selector_text, "button:hover");
        assert_eq!(sheet.rules[1].source_order, 1);
        assert_eq!(decl(&sheet.rules[1].declarations, "border-color"), "red");
        assert!(sheet.rules[1].declarations[0].important);
    }

    #[test]
    fn important_is_case_insensitive() {
        let sheet = parse_stylesheet("button:hover { border-color: red !IMPORTANT }");
        assert_eq!(decl(&sheet.rules[0].declarations, "border-color"), "red");
        assert!(sheet.rules[0].declarations[0].important);
    }

    #[test]
    fn collects_define_color_in_source_order() {
        let sheet = parse_stylesheet(
            "@define-color borders #cdc7c2;\n\
             button { border-color: @borders }\n\
             @define-color accent_color #3584E4;",
        );
        assert_eq!(
            sheet.color_definitions,
            vec![
                ("borders".to_string(), "#cdc7c2".to_string()),
                ("accent_color".to_string(), "#3584E4".to_string()),
            ]
        );
        assert_eq!(
            sheet.rules.len(),
            1,
            "@define-color must not become a style rule"
        );
    }

    #[test]
    fn an_unparseable_rule_does_not_abort_the_sheet() {
        let sheet = parse_stylesheet(
            "@media (min-width: 100px) { button { color: red } }\n\
             button { color: #2e3436 }",
        );
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector_text, "button");
    }

    #[test]
    fn parses_the_whole_vendored_adwaita_sheet() {
        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        assert!(
            sheet.rules.len() > 500,
            "only {} rules parsed out of Adwaita",
            sheet.rules.len()
        );
        assert_eq!(sheet.color_definitions.len(), 37);
        let base = sheet
            .rules
            .iter()
            .find(|r| r.selector_text == "notebook > header > tabs > arrow, button")
            .expect("Adwaita's base `button` rule did not parse");
        assert_eq!(decl(&base.declarations, "border-radius"), "5px");
        assert_eq!(decl(&base.declarations, "padding"), "4px 9px");
        assert_eq!(decl(&base.declarations, "border"), "1px solid");
        assert_eq!(decl(&base.declarations, "border-color"), "#cdc7c2");
        assert_eq!(decl(&base.declarations, "color"), "#2e3436");
        assert_eq!(decl(&base.declarations, "min-height"), "24px");
        assert_eq!(decl(&base.declarations, "min-width"), "16px");
        assert_eq!(
            decl(&base.declarations, "background-image"),
            "linear-gradient(to top, #f6f5f4 2px, #fbfafa)"
        );
    }

    #[test]
    fn comments_inside_values_are_removed_not_kept() {
        // E4: `remaining_text` sliced raw source, so a comment inside a value
        // survived into the value string and broke every later parse.
        let sheet = parse_stylesheet("button { padding: 4px /* x */ 9px }");
        assert_eq!(decl(&sheet.rules[0].declarations, "padding"), "4px 9px");
    }

    #[test]
    fn important_survives_a_trailing_comment() {
        let sheet = parse_stylesheet("button { color: red !important /* c */ }");
        assert_eq!(decl(&sheet.rules[0].declarations, "color"), "red");
        assert!(sheet.rules[0].declarations[0].important);
    }

    #[test]
    fn define_color_survives_a_trailing_comment() {
        let sheet = parse_stylesheet("@define-color accent #3584e4 /* brand */;");
        assert_eq!(
            sheet.color_definitions,
            vec![("accent".to_string(), "#3584e4".to_string())]
        );
    }

    #[test]
    fn whitespace_in_values_is_normalised() {
        let sheet = parse_stylesheet(
            "button { background-image: linear-gradient(\n  to   top,\n  #f6f5f4 2px,\n  #fbfafa) }",
        );
        assert_eq!(
            decl(&sheet.rules[0].declarations, "background-image"),
            "linear-gradient(to top, #f6f5f4 2px, #fbfafa)"
        );
    }

    #[test]
    fn declarations_after_a_nested_block_are_not_swallowed() {
        // E10: the nested rule is discarded, but `color` after it must survive.
        let sheet =
            parse_stylesheet("button { border-radius: 5px; &:hover { color: red } color: blue }");
        assert_eq!(decl(&sheet.rules[0].declarations, "border-radius"), "5px");
        assert_eq!(decl(&sheet.rules[0].declarations, "color"), "blue");
    }

    #[test]
    fn define_color_with_a_block_does_not_leak_a_definition() {
        // E10: the table entry used to be pushed from `parse_prelude`, before
        // the at-rule was known to be valid.
        let sheet = parse_stylesheet("@define-color leaked #fff { }");
        assert!(
            sheet.color_definitions.is_empty(),
            "{:?}",
            sheet.color_definitions
        );
    }

    #[test]
    fn import_is_resolved_relative_to_the_importing_file() {
        let dir = tempdir("import-basic");
        std::fs::write(dir.join("colors.css"), "@define-color accent #3584e4;\n").unwrap();
        std::fs::write(dir.join("widgets.css"), "button { color: @accent }\n").unwrap();
        let sheet = parse_stylesheet_with_base(
            "@import 'colors.css';\n@import url(\"widgets.css\");\nbutton { padding: 1px }",
            Some(&dir),
        );
        assert_eq!(
            sheet.color_definitions,
            vec![("accent".to_string(), "#3584e4".to_string())]
        );
        // Imported rules are spliced in at the import site, so source order
        // -- and therefore the cascade -- matches what a browser would see.
        assert_eq!(sheet.rules.len(), 2);
        assert_eq!(sheet.rules[0].selector_text, "button");
        assert_eq!(decl(&sheet.rules[0].declarations, "color"), "@accent");
        assert_eq!(sheet.rules[0].source_order, 0);
        assert_eq!(decl(&sheet.rules[1].declarations, "padding"), "1px");
        assert_eq!(sheet.rules[1].source_order, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nested_imports_resolve_against_their_own_directory() {
        let dir = tempdir("import-nested");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/leaf.css"), "entry { color: red }").unwrap();
        std::fs::write(dir.join("sub/mid.css"), "@import 'leaf.css';").unwrap();
        let sheet = parse_stylesheet_with_base("@import 'sub/mid.css';", Some(&dir));
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector_text, "entry");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_import_cycle_terminates() {
        let dir = tempdir("import-cycle");
        std::fs::write(dir.join("a.css"), "@import 'b.css';\nbutton { color: red }").unwrap();
        std::fs::write(dir.join("b.css"), "@import 'a.css';\nentry { color: blue }").unwrap();
        let sheet = parse_stylesheet_with_base("@import 'a.css';", Some(&dir));
        assert_eq!(sheet.rules.len(), 2, "{:?}", sheet.rules);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resource_urls_and_base_less_sheets_skip_imports_without_dropping_the_rest() {
        let sheet = parse_stylesheet(
            "@import url(\"resource:///org/gtk/libgtk/theme/Default/Default-light.css\");\n             button { color: red }",
        );
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(decl(&sheet.rules[0].declarations, "color"), "red");
    }

    #[test]
    fn a_missing_import_target_does_not_abort_the_sheet() {
        let dir = tempdir("import-missing");
        let sheet =
            parse_stylesheet_with_base("@import 'nope.css';\nbutton { color: red }", Some(&dir));
        assert_eq!(sheet.rules.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
