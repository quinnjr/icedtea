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
use std::rc::Rc;

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

/// The viewer state `@media` queries evaluate against.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MediaEnv {
    /// `prefers-color-scheme`.
    pub color_scheme: ColorScheme,
    /// `prefers-contrast`.
    pub contrast: Contrast,
}

impl Default for MediaEnv {
    fn default() -> Self {
        MediaEnv {
            color_scheme: ColorScheme::Light,
            contrast: Contrast::NoPreference,
        }
    }
}

/// `prefers-color-scheme`'s two values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    /// `light`.
    Light,
    /// `dark`.
    Dark,
}

/// `prefers-contrast`'s three values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Contrast {
    /// `no-preference`.
    NoPreference,
    /// `more`.
    More,
    /// `less`.
    Less,
}

/// A parsed media condition.
///
/// `prefers-reduced-motion` is a real GTK 4.20+ feature; it parses here and
/// always evaluates **false**, because `MediaEnv` does not model it in M2.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaQuery {
    /// `(prefers-color-scheme: ...)`.
    ColorScheme(ColorScheme),
    /// `(prefers-contrast: ...)`.
    Contrast(Contrast),
    /// `(prefers-reduced-motion: reduce)`.
    ReducedMotion,
    /// `not <query>`.
    Not(Rc<MediaQuery>),
    /// `<query> and <query> ...`.
    And(Rc<[MediaQuery]>),
    /// `<query>, <query> ...` / `<query> or <query>`.
    Or(Rc<[MediaQuery]>),
    /// An unknown feature: parses, never matches.
    AlwaysFalse,
}

impl MediaQuery {
    /// Whether this query matches `env`.
    #[must_use]
    pub fn evaluate(&self, env: &MediaEnv) -> bool {
        match self {
            MediaQuery::ColorScheme(scheme) => env.color_scheme == *scheme,
            MediaQuery::Contrast(contrast) => env.contrast == *contrast,
            MediaQuery::ReducedMotion => false,
            MediaQuery::Not(inner) => !inner.evaluate(env),
            MediaQuery::And(list) => list.iter().all(|query| query.evaluate(env)),
            MediaQuery::Or(list) => list.iter().any(|query| query.evaluate(env)),
            MediaQuery::AlwaysFalse => false,
        }
    }
}

/// One `@media` block, kept **unevaluated** so a single parse can be
/// compiled under several environments.
#[derive(Clone, Debug)]
pub struct MediaBlock {
    /// The block's condition.
    pub query: MediaQuery,
    /// Its qualified rules, numbered in the outer sheet's source order.
    pub rules: Vec<StyleRule>,
    /// Its `@keyframes`.
    pub keyframes: Vec<KeyframesRule>,
    /// Its `@define-color`s.
    pub color_definitions: Vec<(String, String)>,
}

/// One `@keyframes` rule, with raw declarations; compiled in `cascade.rs`.
#[derive(Clone, Debug)]
pub struct KeyframesRule {
    /// The animation name.
    pub name: String,
    /// `(offsets in 0..=1, declarations)`, in source order.
    pub frames: Vec<(Vec<f32>, Vec<Declaration>)>,
    /// Position in the sheet; the cascade's last-definition-wins tiebreak.
    pub source_order: usize,
}

/// A parsed stylesheet.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    /// Qualified rules, in source order (imports spliced in at their site).
    pub rules: Vec<StyleRule>,
    /// `(name, value)` for every `@define-color`, in source order.
    pub color_definitions: Vec<(String, String)>,
    /// Every `@keyframes`, in source order.
    pub keyframes: Vec<KeyframesRule>,
    /// Every `@media` block, unevaluated.
    pub media_blocks: Vec<MediaBlock>,
}

impl Stylesheet {
    /// Append `layer` after `self`, renumbering so the later layer wins
    /// every source-order tie.
    pub fn append_layer(&mut self, layer: Stylesheet) {
        let offset = next_source_order(self);
        let mut base = layer;
        for rule in &mut base.rules {
            rule.source_order += offset;
        }
        for keyframes in &mut base.keyframes {
            keyframes.source_order += offset;
        }
        for block in &mut base.media_blocks {
            for rule in &mut block.rules {
                rule.source_order += offset;
            }
            for keyframes in &mut block.keyframes {
                keyframes.source_order += offset;
            }
        }
        self.rules.extend(base.rules);
        self.color_definitions.extend(base.color_definitions);
        self.keyframes.extend(base.keyframes);
        self.media_blocks.extend(base.media_blocks);
    }
}

/// The next unused source-order slot in `sheet`.
///
/// Source order is global across top-level rules, `@keyframes` and the
/// rules inside `@media` blocks, so splicing a matching block back in at
/// compile time restores document order exactly.
fn next_source_order(sheet: &Stylesheet) -> usize {
    let rules = sheet
        .rules
        .iter()
        .map(|rule| rule.source_order + 1)
        .max()
        .unwrap_or(0);
    let keyframes = sheet
        .keyframes
        .iter()
        .map(|frame| frame.source_order + 1)
        .max()
        .unwrap_or(0);
    let nested = sheet
        .media_blocks
        .iter()
        .flat_map(|block| {
            block
                .rules
                .iter()
                .map(|rule| rule.source_order + 1)
                .chain(block.keyframes.iter().map(|frame| frame.source_order + 1))
        })
        .max()
        .unwrap_or(0);
    rules.max(keyframes).max(nested)
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
    /// `@keyframes <name> { ... }`.
    Keyframes(KeyframesRule),
    /// `@media <query> { ... }`.
    Media(MediaBlock),
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
    Keyframes(String),
    Media(MediaQuery),
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
        if name.eq_ignore_ascii_case("keyframes") {
            let animation = input.expect_ident()?.as_ref().to_string();
            input.skip_whitespace();
            if !input.is_exhausted() {
                return Err(input.new_custom_error(()));
            }
            return Ok(AtPrelude::Keyframes(animation));
        }
        if name.eq_ignore_ascii_case("media") {
            let query =
                parse_media_query_list(input).map_err(|()| input.new_custom_error::<(), ()>(()))?;
            return Ok(AtPrelude::Media(query));
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

    fn parse_block<'t>(
        &mut self,
        prelude: AtPrelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<SheetItem, ParseError<'i, ()>> {
        match prelude {
            AtPrelude::Keyframes(name) => {
                let frames = parse_keyframe_list(input);
                Ok(SheetItem::Keyframes(KeyframesRule {
                    name,
                    frames,
                    source_order: 0,
                }))
            }
            AtPrelude::Media(query) => {
                let mut nested = Stylesheet::default();
                let mut sheet_parser = SheetParser;
                for item in StyleSheetParser::new(input, &mut sheet_parser) {
                    match item {
                        Ok(SheetItem::Rule(rule)) => nested.rules.push(rule),
                        Ok(SheetItem::Color(name, value)) => {
                            nested.color_definitions.push((name, value));
                        }
                        Ok(SheetItem::Keyframes(frames)) => nested.keyframes.push(frames),
                        Ok(SheetItem::Media(_)) => {
                            tracing::debug!("nested @media is not modelled; dropping the block");
                        }
                        Ok(SheetItem::Import(url)) => {
                            tracing::debug!(%url, "@import inside @media; skipping");
                        }
                        Ok(SheetItem::Ignored) => {}
                        Err((err, slice)) => {
                            tracing::debug!(?err, rule = %slice.chars().take(80).collect::<String>(), "skipping invalid CSS rule inside @media");
                        }
                    }
                }
                Ok(SheetItem::Media(MediaBlock {
                    query,
                    rules: nested.rules,
                    keyframes: nested.keyframes,
                    color_definitions: nested.color_definitions,
                }))
            }
            // `@import`/`@define-color` carry no block, so a block means the
            // rule was malformed: the definition is *not* recorded.
            AtPrelude::Color(_, _) | AtPrelude::Import(_) => {
                while input.next().is_ok() {}
                Ok(SheetItem::Ignored)
            }
        }
    }

    fn rule_without_block(
        &mut self,
        prelude: AtPrelude,
        _start: &ParserState,
    ) -> Result<SheetItem, ()> {
        Ok(match prelude {
            AtPrelude::Color(name, value) => SheetItem::Color(name, value),
            AtPrelude::Import(url) => SheetItem::Import(url),
            // `@keyframes`/`@media` are block-only: without one they are
            // malformed and carry nothing.
            AtPrelude::Keyframes(_) | AtPrelude::Media(_) => SheetItem::Ignored,
        })
    }
}

/// `<media-query-list>`, restricted to the features GTK documents.
fn parse_media_query_list(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let mut queries = vec![parse_media_condition(input)?];
    while input.expect_comma().is_ok() {
        queries.push(parse_media_condition(input)?);
    }
    input.skip_whitespace();
    if !input.is_exhausted() {
        return Err(());
    }
    Ok(if queries.len() == 1 {
        queries.remove(0)
    } else {
        MediaQuery::Or(queries.into())
    })
}

fn parse_media_condition(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let state = input.state();
    let negated = matches!(
        input.next(),
        Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("not")
    );
    if negated {
        return Ok(MediaQuery::Not(Rc::new(parse_media_condition(input)?)));
    }
    input.reset(&state);
    let first = parse_media_in_parens(input)?;
    let mut combinator: Option<bool> = None; // Some(true) == `and`
    let mut operands = vec![first];
    loop {
        let state = input.state();
        let keyword = match input.next() {
            Ok(Token::Ident(name)) => Some(name.as_ref().to_ascii_lowercase()),
            _ => None,
        };
        let Some(keyword) = keyword else {
            input.reset(&state);
            break;
        };
        let is_and = if keyword == "and" {
            true
        } else if keyword == "or" {
            false
        } else {
            input.reset(&state);
            break;
        };
        if combinator.is_some_and(|previous| previous != is_and) {
            return Err(());
        }
        combinator = Some(is_and);
        operands.push(parse_media_in_parens(input)?);
    }
    Ok(match combinator {
        None => operands.remove(0),
        Some(true) => MediaQuery::And(operands.into()),
        Some(false) => MediaQuery::Or(operands.into()),
    })
}

fn parse_media_in_parens(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    input.expect_parenthesis_block().map_err(|_| ())?;
    input
        .parse_nested_block(|inner| match parse_media_feature(inner) {
            Ok(query) => Ok(query),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: ParseError<'_, ()>| ())
}

fn parse_media_feature(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    // `( <condition> )` nests rather than naming a feature.
    let state = input.state();
    if let Ok(nested) = parse_media_condition(input) {
        input.skip_whitespace();
        if input.is_exhausted() {
            return Ok(nested);
        }
    }
    input.reset(&state);
    let feature = input
        .expect_ident()
        .map_err(|_| ())?
        .as_ref()
        .to_ascii_lowercase();
    input.expect_colon().map_err(|_| ())?;
    // Every feature this engine models takes a keyword. A well-formed
    // feature whose value is not one -- `(min-width: 100px)` -- is a
    // feature we do not model, so it parses and never matches rather than
    // discarding the whole block.
    let value = input
        .expect_ident()
        .map(|value| value.as_ref().to_ascii_lowercase())
        .map_err(|_| ());
    let Ok(value) = value else {
        while input.next().is_ok() {}
        return Ok(MediaQuery::AlwaysFalse);
    };
    input.skip_whitespace();
    if !input.is_exhausted() {
        return Err(());
    }
    Ok(match (feature.as_str(), value.as_str()) {
        ("prefers-color-scheme", "light") => MediaQuery::ColorScheme(ColorScheme::Light),
        ("prefers-color-scheme", "dark") => MediaQuery::ColorScheme(ColorScheme::Dark),
        ("prefers-contrast", "no-preference") => MediaQuery::Contrast(Contrast::NoPreference),
        ("prefers-contrast", "more") => MediaQuery::Contrast(Contrast::More),
        ("prefers-contrast", "less") => MediaQuery::Contrast(Contrast::Less),
        // GTK 4.20+ supports this feature; M2's MediaEnv does not model it,
        // so `reduce` never matches and `no-preference` always does.
        ("prefers-reduced-motion", "reduce" | "reduced") => MediaQuery::ReducedMotion,
        ("prefers-reduced-motion", "no-preference") => {
            MediaQuery::Not(Rc::new(MediaQuery::ReducedMotion))
        }
        _ => MediaQuery::AlwaysFalse,
    })
}

/// The `<keyframe-selector>#` prelude: `from`, `to` or `<percentage>`.
struct KeyframeListParser;

impl<'i> QualifiedRuleParser<'i> for KeyframeListParser {
    type Prelude = Vec<f32>;
    type QualifiedRule = (Vec<f32>, Vec<Declaration>);
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Vec<f32>, ParseError<'i, ()>> {
        let mut offsets = Vec::new();
        loop {
            let token = input.next()?.clone();
            let offset = match token {
                Token::Percentage { unit_value, .. } if unit_value.is_finite() => unit_value,
                Token::Ident(ref name) if name.eq_ignore_ascii_case("from") => 0.0,
                Token::Ident(ref name) if name.eq_ignore_ascii_case("to") => 1.0,
                _ => return Err(input.new_custom_error(())),
            };
            offsets.push(offset);
            if input.expect_comma().is_err() {
                break;
            }
        }
        input.skip_whitespace();
        if !input.is_exhausted() {
            return Err(input.new_custom_error(()));
        }
        Ok(offsets)
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Vec<f32>,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(Vec<f32>, Vec<Declaration>), ParseError<'i, ()>> {
        let mut block = DeclarationBlockParser;
        let declarations: Vec<Declaration> = RuleBodyParser::<_, _, ()>::new(input, &mut block)
            .flatten()
            .flatten()
            .collect();
        Ok((prelude, declarations))
    }
}

impl<'i> AtRuleParser<'i> for KeyframeListParser {
    type Prelude = ();
    type AtRule = (Vec<f32>, Vec<Declaration>);
    type Error = ();
}

fn parse_keyframe_list(input: &mut Parser<'_, '_>) -> Vec<(Vec<f32>, Vec<Declaration>)> {
    let mut parser = KeyframeListParser;
    StyleSheetParser::new(input, &mut parser)
        .filter_map(|item| match item {
            Ok(frame) => Some(frame),
            Err((err, slice)) => {
                tracing::debug!(?err, frame = %slice.chars().take(80).collect::<String>(), "skipping invalid keyframe");
                None
            }
        })
        .collect()
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
                rule.source_order = next_source_order(out);
                out.rules.push(rule);
            }
            Ok(SheetItem::Color(name, value)) => out.color_definitions.push((name, value)),
            Ok(SheetItem::Import(url)) => resolve_import(&url, base_dir, depth, visited, out),
            Ok(SheetItem::Keyframes(mut frames)) => {
                frames.source_order = next_source_order(out);
                out.keyframes.push(frames);
            }
            Ok(SheetItem::Media(mut block)) => {
                let base = next_source_order(out);
                for (index, rule) in block.rules.iter_mut().enumerate() {
                    rule.source_order = base + index;
                }
                for (index, keyframes) in block.keyframes.iter_mut().enumerate() {
                    keyframes.source_order = base + block.rules.len() + index;
                }
                out.media_blocks.push(block);
            }
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

    use super::{ColorScheme, Contrast, MediaEnv, MediaQuery};

    #[test]
    fn keyframes_are_collected_and_do_not_become_rules() {
        let sheet = parse_stylesheet(
            "@keyframes spin { to { transform: rotate(1turn); } }\n\
             button { color: red }",
        );
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.keyframes.len(), 1);
        assert_eq!(sheet.keyframes[0].name, "spin");
        assert_eq!(sheet.keyframes[0].frames.len(), 1);
        assert_eq!(sheet.keyframes[0].frames[0].0, vec![1.0]);
        assert_eq!(
            decl(&sheet.keyframes[0].frames[0].1, "transform"),
            "rotate(1turn)"
        );
    }

    #[test]
    fn keyframe_selectors_accept_from_to_percentages_and_comma_lists() {
        let sheet = parse_stylesheet(
            "@keyframes fade { from { opacity: 0 } 25%, 75% { opacity: 0.5 } to { opacity: 1 } }",
        );
        let frames = &sheet.keyframes[0].frames;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].0, vec![0.0]);
        assert_eq!(frames[1].0, vec![0.25, 0.75]);
        assert_eq!(frames[2].0, vec![1.0]);
    }

    #[test]
    fn adwaita_carries_exactly_two_keyframes_rules_and_still_900_style_rules() {
        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        assert_eq!(sheet.keyframes.len(), 2);
        let names: Vec<&str> = sheet.keyframes.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"spin"), "{names:?}");
        assert!(names.contains(&"needs_attention"), "{names:?}");
        assert!(sheet.media_blocks.is_empty());
        assert_eq!(sheet.color_definitions.len(), 37);
    }

    #[test]
    fn media_blocks_are_retained_unevaluated() {
        let sheet = parse_stylesheet(
            "@media (prefers-color-scheme: dark) { button { color: white } }\n\
             button { color: black }",
        );
        // The block's rules are NOT spliced into `rules`; the compiler
        // decides, per MediaEnv.
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.media_blocks.len(), 1);
        assert_eq!(sheet.media_blocks[0].rules.len(), 1);
        assert_eq!(
            sheet.media_blocks[0].query,
            MediaQuery::ColorScheme(ColorScheme::Dark)
        );
        // Source order is global, so a splice keeps document order.
        assert!(sheet.media_blocks[0].rules[0].source_order < sheet.rules[0].source_order);
    }

    #[test]
    fn media_queries_evaluate_against_the_environment() {
        let light = MediaEnv::default();
        let dark = MediaEnv {
            color_scheme: ColorScheme::Dark,
            contrast: Contrast::NoPreference,
        };
        let hc = MediaEnv {
            color_scheme: ColorScheme::Light,
            contrast: Contrast::More,
        };
        assert_eq!(
            light,
            MediaEnv {
                color_scheme: ColorScheme::Light,
                contrast: Contrast::NoPreference
            }
        );
        assert!(MediaQuery::ColorScheme(ColorScheme::Light).evaluate(&light));
        assert!(!MediaQuery::ColorScheme(ColorScheme::Light).evaluate(&dark));
        assert!(MediaQuery::Contrast(Contrast::More).evaluate(&hc));
        assert!(!MediaQuery::Contrast(Contrast::More).evaluate(&light));
        // `prefers-reduced-motion: reduce` parses and never matches in M2.
        assert!(!MediaQuery::ReducedMotion.evaluate(&light));
        assert!(!MediaQuery::AlwaysFalse.evaluate(&light));
        let not_dark =
            MediaQuery::Not(std::rc::Rc::new(MediaQuery::ColorScheme(ColorScheme::Dark)));
        assert!(not_dark.evaluate(&light));
        let both = MediaQuery::And(std::rc::Rc::from(vec![
            MediaQuery::ColorScheme(ColorScheme::Light),
            MediaQuery::Contrast(Contrast::NoPreference),
        ]));
        assert!(both.evaluate(&light));
        let either = MediaQuery::Or(std::rc::Rc::from(vec![
            MediaQuery::ColorScheme(ColorScheme::Dark),
            MediaQuery::Contrast(Contrast::NoPreference),
        ]));
        assert!(either.evaluate(&light));
    }

    #[test]
    fn the_media_prelude_grammar_covers_gtks_features_and_combinators() {
        let query = |text: &str| {
            let css = format!("@media {text} {{ button {{ color: red }} }}");
            parse_stylesheet(&css)
                .media_blocks
                .first()
                .map(|block| block.query.clone())
        };
        assert_eq!(
            query("(prefers-color-scheme: light)"),
            Some(MediaQuery::ColorScheme(ColorScheme::Light))
        );
        assert_eq!(
            query("(PREFERS-CONTRAST: more)"),
            Some(MediaQuery::Contrast(Contrast::More))
        );
        assert_eq!(
            query("(prefers-reduced-motion: reduce)"),
            Some(MediaQuery::ReducedMotion)
        );
        assert_eq!(
            query("(prefers-reduced-motion: no-preference)"),
            Some(MediaQuery::Not(std::rc::Rc::new(MediaQuery::ReducedMotion)))
        );
        assert!(matches!(
            query("not (prefers-color-scheme: dark)"),
            Some(MediaQuery::Not(_))
        ));
        assert!(matches!(
            query("(prefers-color-scheme: dark) and (prefers-contrast: more)"),
            Some(MediaQuery::And(_))
        ));
        assert!(matches!(
            query("(prefers-color-scheme: dark), (prefers-contrast: more)"),
            Some(MediaQuery::Or(_))
        ));
        // An unknown feature parses and never matches, so its block is
        // fully parsed and then simply never applies.
        assert_eq!(query("(min-width: 100px)"), Some(MediaQuery::AlwaysFalse));
    }

    #[test]
    fn appending_a_layer_carries_keyframes_and_media_blocks_too() {
        let mut base = parse_stylesheet("button { color: red }");
        let layer = parse_stylesheet(
            "@keyframes spin { to { opacity: 0 } }\n\
             @media (prefers-contrast: more) { button { color: white } }\n\
             button { color: blue }",
        );
        base.append_layer(layer);
        assert_eq!(base.rules.len(), 2);
        assert_eq!(base.keyframes.len(), 1);
        assert_eq!(base.media_blocks.len(), 1);
        // The layered rule must sort after the base rule.
        assert!(base.rules[1].source_order > base.rules[0].source_order);
        assert!(base.media_blocks[0].rules[0].source_order > base.rules[0].source_order);
    }

    #[test]
    fn a_malformed_at_rule_still_does_not_abort_the_sheet() {
        let sheet = parse_stylesheet(
            "@keyframes { to { opacity: 0 } }\n\
             @media { button { color: white } }\n\
             button { color: black }",
        );
        assert_eq!(sheet.rules.len(), 1);
        assert!(sheet.keyframes.is_empty());
        assert!(sheet.media_blocks.is_empty());
    }
}
