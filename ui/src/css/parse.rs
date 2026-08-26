//! `cssparser` glue: CSS text in, `(selector text, declarations)` rules and
//! raw `@define-color` pairs out.
//!
//! Deliberately value-agnostic: declaration values are kept as their
//! verbatim source text and interpreted later (`super::colors`,
//! `super::computed`). That keeps every GTK-specific value form
//! (`image()`, `-gtk-*`, relative color syntax) parseable-as-text even
//! when this milestone cannot yet interpret it.

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser,
};

/// One `name: value` pair from a declaration block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// Property name, ASCII-lowercased.
    pub name: String,
    /// Verbatim value text, `!important` stripped and trimmed.
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
    /// Qualified rules, in source order.
    pub rules: Vec<StyleRule>,
    /// `(name, raw value)` for every `@define-color`, in source order.
    pub color_definitions: Vec<(String, String)>,
}

/// Split a raw declaration value into `(value, important)`.
///
/// Tolerates whitespace between `!` and `important` (`cssparser` tokenizes
/// them as separate tokens, so the source text can carry either spelling).
fn split_important(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    // `to_ascii_lowercase` only rewrites ASCII bytes, so it never changes the
    // string's length or moves a byte off a UTF-8 char boundary: the length
    // it reports lines up with `trimmed`'s own indices.
    let lower = trimmed.to_ascii_lowercase();
    if let Some(head_len) = lower.strip_suffix("important").map(str::len) {
        let head = trimmed[..head_len].trim_end();
        if let Some(head) = head.strip_suffix('!') {
            return (head.trim_end().to_string(), true);
        }
    }
    (trimmed.to_string(), false)
}

/// Consume every remaining token in `input` and return the source text it spanned.
fn remaining_text<'i>(input: &mut Parser<'i, '_>) -> &'i str {
    let start = input.position();
    while input.next().is_ok() {}
    input.slice_from(start)
}

/// Parses a single declaration block. `RuleBodyItemParser` demands one type
/// for declarations, nested qualified rules and nested at-rules alike; nested
/// rules are refused (`parse_qualified` is `false`), so `Declaration` serves
/// as all three.
struct DeclarationBlockParser;

impl<'i> DeclarationParser<'i> for DeclarationBlockParser {
    type Declaration = Declaration;
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _start: &ParserState,
    ) -> Result<Declaration, ParseError<'i, ()>> {
        let (value, important) = split_important(remaining_text(input));
        Ok(Declaration {
            name: name.as_ref().to_ascii_lowercase(),
            value,
            important,
        })
    }
}

impl<'i> AtRuleParser<'i> for DeclarationBlockParser {
    type Prelude = ();
    type AtRule = Declaration;
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for DeclarationBlockParser {
    type Prelude = ();
    type QualifiedRule = Declaration;
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, Declaration, ()> for DeclarationBlockParser {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// Top-level rule-list parser. `StyleSheetParser` unifies qualified rules and
/// at-rules under one type, so both yield `Option<StyleRule>`: `Some` for a
/// style rule, `None` for an `@define-color` (whose payload is pushed onto
/// `color_definitions` instead).
struct SheetParser {
    color_definitions: Vec<(String, String)>,
}

impl<'i> QualifiedRuleParser<'i> for SheetParser {
    type Prelude = String;
    type QualifiedRule = Option<StyleRule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<String, ParseError<'i, ()>> {
        Ok(remaining_text(input).trim().to_string())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: String,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Option<StyleRule>, ParseError<'i, ()>> {
        let mut block = DeclarationBlockParser;
        // A malformed declaration must not discard the rest of the block --
        // GTK themes routinely carry properties this milestone's tokenizer
        // path cannot interpret, so errors are simply dropped (`.flatten()`).
        let declarations: Vec<Declaration> = RuleBodyParser::<_, _, ()>::new(input, &mut block)
            .flatten()
            .collect();
        Ok(Some(StyleRule {
            selector_text: prelude,
            declarations,
            // Filled in by `parse_stylesheet`, which knows the index.
            source_order: 0,
        }))
    }
}

impl<'i> AtRuleParser<'i> for SheetParser {
    type Prelude = ();
    type AtRule = Option<StyleRule>;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        if !name.eq_ignore_ascii_case("define-color") {
            return Err(input.new_custom_error(()));
        }
        let color_name = input.expect_ident()?.as_ref().to_string();
        let value = remaining_text(input).trim().to_string();
        if value.is_empty() {
            return Err(input.new_custom_error(()));
        }
        self.color_definitions.push((color_name, value));
        Ok(())
    }

    fn rule_without_block(
        &mut self,
        _prelude: (),
        _start: &ParserState,
    ) -> Result<Option<StyleRule>, ()> {
        Ok(None)
    }
}

/// Parse `css` into rules and `@define-color` pairs.
///
/// Invalid rules are skipped rather than aborting the sheet, matching CSS's
/// own error-recovery rules and GTK's tolerance of unknown syntax.
#[must_use]
pub fn parse_stylesheet(css: &str) -> Stylesheet {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut sheet_parser = SheetParser {
        color_definitions: Vec::new(),
    };
    let mut rules: Vec<StyleRule> = Vec::new();
    for item in StyleSheetParser::new(&mut parser, &mut sheet_parser) {
        match item {
            Ok(Some(mut rule)) => {
                rule.source_order = rules.len();
                rules.push(rule);
            }
            Ok(None) => {}
            Err((err, slice)) => {
                tracing::debug!(?err, rule = %slice.chars().take(80).collect::<String>(), "skipping invalid CSS rule");
            }
        }
    }
    Stylesheet {
        rules,
        color_definitions: sheet_parser.color_definitions,
    }
}

#[cfg(test)]
mod tests {
    use super::{Declaration, parse_stylesheet};

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
}
