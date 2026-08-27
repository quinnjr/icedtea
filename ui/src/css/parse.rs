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

use super::depth_guard::DepthGuard;
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
    /// 0-based position of this rule among the sheet's *items* -- rules,
    /// `@keyframes` and `@define-color`s all draw from one counter -- which
    /// is the cascade's final tiebreak. Only the ordering is meaningful; the
    /// numbers are not a rule index.
    pub source_order: usize,
    /// Which layer this rule came from: 0 is the base sheet, and each
    /// [`Stylesheet::append_layer`] adds one. The cascade ranks a higher
    /// origin above *any* specificity, the way GTK ranks a
    /// `GTK_STYLE_PROVIDER_PRIORITY_USER` provider above a theme one.
    pub origin: u8,
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
    /// A `<media-type>` this engine has already decided about: `screen` and
    /// `all` are `true`, `print` and friends `false`. Unlike
    /// [`MediaQuery::Unknown`] this is a *definite* answer, so `not print`
    /// matches.
    Always(bool),
    /// A feature this engine does not model. Media Queries 4 gives such a
    /// query the third truth value: it matches nothing, and negating it
    /// still matches nothing.
    Unknown,
}

impl MediaQuery {
    /// Whether this query matches `env`.
    #[must_use]
    pub fn evaluate(&self, env: &MediaEnv) -> bool {
        self.evaluate3(env) == Some(true)
    }

    /// Media Queries 4's three-valued evaluation: `None` is *unknown*.
    ///
    /// The third value is what keeps `@media not (forced-colors: active)`
    /// from applying unconditionally. `not unknown` is unknown; `unknown and
    /// false` is false and `unknown or true` is true, because those are
    /// decided by the other operand alone.
    fn evaluate3(&self, env: &MediaEnv) -> Option<bool> {
        match self {
            MediaQuery::ColorScheme(scheme) => Some(env.color_scheme == *scheme),
            MediaQuery::Contrast(contrast) => Some(env.contrast == *contrast),
            MediaQuery::ReducedMotion => Some(false),
            MediaQuery::Always(answer) => Some(*answer),
            MediaQuery::Unknown => None,
            MediaQuery::Not(inner) => inner.evaluate3(env).map(|value| !value),
            MediaQuery::And(list) => {
                let mut unknown = false;
                for query in list.iter() {
                    match query.evaluate3(env) {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => unknown = true,
                    }
                }
                (!unknown).then_some(true)
            }
            MediaQuery::Or(list) => {
                let mut unknown = false;
                for query in list.iter() {
                    match query.evaluate3(env) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => unknown = true,
                    }
                }
                (!unknown).then_some(false)
            }
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
    /// The source-order slot of each entry of `color_definitions`, parallel
    /// to it. Kept alongside rather than folded into the tuple so the pair
    /// stays `(name, value)` for every existing consumer.
    pub color_definition_orders: Vec<usize>,
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
    /// The source-order slot of each entry of `color_definitions`, parallel
    /// to it.
    ///
    /// A definition inside a matching `@media` block has to be spliced back
    /// in at *its own* position, not appended after every top-level one, or
    /// `@media (...) { @define-color bg black }` beats a later top-level
    /// `@define-color bg white` that GTK would have let win.
    pub color_definition_orders: Vec<usize>,
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
        let origin = self
            .rules
            .iter()
            .map(|rule| rule.origin)
            .chain(
                self.media_blocks
                    .iter()
                    .flat_map(|block| block.rules.iter().map(|rule| rule.origin)),
            )
            .max()
            .map_or(1, |highest| highest.saturating_add(1));
        for rule in &mut base.rules {
            rule.source_order += offset;
            rule.origin = origin;
        }
        for order in &mut base.color_definition_orders {
            *order += offset;
        }
        for keyframes in &mut base.keyframes {
            keyframes.source_order += offset;
        }
        for block in &mut base.media_blocks {
            for rule in &mut block.rules {
                rule.source_order += offset;
                rule.origin = origin;
            }
            for keyframes in &mut block.keyframes {
                keyframes.source_order += offset;
            }
            for order in &mut block.color_definition_orders {
                *order += offset;
            }
        }
        self.rules.extend(base.rules);
        self.color_definitions.extend(base.color_definitions);
        self.color_definition_orders
            .extend(base.color_definition_orders);
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
    let colors = sheet
        .color_definition_orders
        .iter()
        .chain(
            sheet
                .media_blocks
                .iter()
                .flat_map(|block| block.color_definition_orders.iter()),
        )
        .map(|order| order + 1)
        .max()
        .unwrap_or(0);
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
    rules.max(keyframes).max(nested).max(colors)
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
///
/// `inside_media` says whether this parser is already running *inside* an
/// `@media` block. A nested `@media` carries nothing (the engine does not
/// model one, and its whole block has always been dropped), so instead of
/// recursing into it -- `parse_block` -> `StyleSheetParser` -> `parse_block`,
/// once per level, which a hostile `gtk.css` could drive until the stack
/// overflowed and the process *aborted* -- the nested block's tokens are
/// skipped without ever growing the stack.
struct SheetParser {
    inside_media: bool,
}

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
            origin: 0,
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
            AtPrelude::Media(_) if self.inside_media => {
                // A nested `@media` is not modelled, and its block has always
                // been dropped whole. Skipping its tokens here -- rather than
                // parsing them through another `StyleSheetParser` first --
                // keeps the result identical while making the depth of the
                // nesting cost no stack at all: `@media (...) {` repeated a
                // few hundred times used to overflow the stack and abort.
                tracing::debug!("nested @media is not modelled; dropping the block");
                while input.next().is_ok() {}
                Ok(SheetItem::Ignored)
            }
            AtPrelude::Media(query) => {
                let mut nested = Stylesheet::default();
                let mut sheet_parser = SheetParser { inside_media: true };
                for item in StyleSheetParser::new(input, &mut sheet_parser) {
                    match item {
                        Ok(SheetItem::Rule(rule)) => nested.rules.push(rule),
                        Ok(SheetItem::Color(name, value)) => {
                            nested.color_definitions.push((name, value));
                        }
                        Ok(SheetItem::Keyframes(frames)) => nested.keyframes.push(frames),
                        Ok(SheetItem::Import(url)) => {
                            tracing::debug!(%url, "@import inside @media; skipping");
                        }
                        // A nested `@media` arrives as `Ignored`: this
                        // parser has `inside_media` set, so it skipped the
                        // block rather than building a `Media` item from it.
                        Ok(SheetItem::Ignored | SheetItem::Media(_)) => {}
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
                    // Filled in by the caller, which knows where the block
                    // itself sits in the outer sheet.
                    color_definition_orders: Vec::new(),
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

/// Consume a comma if the next token is one, leaving the parser untouched
/// otherwise.
///
/// `cssparser`'s `expect_comma()` is built on `next()`, which consumes the
/// token it then rejects; used as a loop condition it silently eats one
/// stray token and accepts the truncated production. Every "an optional
/// comma continues the list" site has to reset instead.
fn eat_comma(input: &mut Parser<'_, '_>) -> bool {
    let state = input.state();
    if matches!(input.next(), Ok(Token::Comma)) {
        true
    } else {
        input.reset(&state);
        false
    }
}

/// `<media-query-list>`, restricted to the features GTK documents.
fn parse_media_query_list(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let mut queries = vec![parse_media_query(input)?];
    while eat_comma(input) {
        queries.push(parse_media_query(input)?);
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

/// One `<media-query>`: either a bare `<media-condition>`, or Media Queries
/// 4's `[ not | only ]? <media-type> [ and <media-condition> ]?` form.
///
/// The type form is why `@media screen { ... }` has to parse at all: without
/// it the prelude failed and the whole block was dropped, where MQ4 says an
/// unrecognised *type* merely makes the query `not all` -- the block stays,
/// it just never matches. `screen` and `all` do match here; a GTK widget is
/// on a screen.
fn parse_media_query(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let state = input.state();
    if let Ok(query) = parse_media_type_query(input) {
        return Ok(query);
    }
    input.reset(&state);
    parse_media_condition(input)
}

fn parse_media_type_query(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    let mut negated = false;
    let first = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    let name = if first.eq_ignore_ascii_case("not") || first.eq_ignore_ascii_case("only") {
        negated = first.eq_ignore_ascii_case("not");
        // `not (feature)` is a condition, not a type query: only an ident
        // may follow the modifier here.
        input.expect_ident().map_err(|_| ())?.as_ref().to_string()
    } else {
        first
    };
    // `and`/`or` are combinators, never types.
    if name.eq_ignore_ascii_case("and") || name.eq_ignore_ascii_case("or") {
        return Err(());
    }
    let matches_type = name.eq_ignore_ascii_case("all") || name.eq_ignore_ascii_case("screen");
    let mut query = MediaQuery::Always(matches_type);
    let after_type = input.state();
    if matches!(input.next(), Ok(Token::Ident(word)) if word.eq_ignore_ascii_case("and")) {
        let condition = parse_media_condition(input)?;
        query = MediaQuery::And(vec![query, condition].into());
    } else {
        input.reset(&after_type);
    }
    Ok(if negated {
        MediaQuery::Not(Rc::new(query))
    } else {
        query
    })
}

fn parse_media_condition(input: &mut Parser<'_, '_>) -> Result<MediaQuery, ()> {
    // Every recursive entry point in this grammar -- a leading `not`, a
    // parenthesized group, a nested condition inside a feature -- runs
    // through here or `parse_media_in_parens`, so one guard at each bounds
    // the whole prelude. `@media` + 15000 `(` and `not not not ...` both
    // overflowed the stack before.
    let _guard = DepthGuard::enter().ok_or(())?;
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
    let _guard = DepthGuard::enter().ok_or(())?;
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
        return Ok(MediaQuery::Unknown);
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
        _ => MediaQuery::Unknown,
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
            // A resetting comma: `expect_comma()` would swallow the offending
            // token, so `0% 50% { ... }` was accepted with the `50%` dropped.
            let after_offset = input.state();
            if !matches!(input.next(), Ok(Token::Comma)) {
                input.reset(&after_offset);
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
    let mut sheet_parser = SheetParser {
        inside_media: false,
    };
    for item in StyleSheetParser::new(&mut parser, &mut sheet_parser) {
        match item {
            Ok(SheetItem::Rule(mut rule)) => {
                rule.source_order = next_source_order(out);
                out.rules.push(rule);
            }
            Ok(SheetItem::Color(name, value)) => {
                let order = next_source_order(out);
                out.color_definitions.push((name, value));
                out.color_definition_orders.push(order);
            }
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
                // A definition inside the block sits where the *block*
                // sits, after its rules and keyframes, so an earlier
                // top-level definition loses to it and a later one wins.
                let after = base + block.rules.len() + block.keyframes.len();
                block.color_definition_orders = (0..block.color_definitions.len())
                    .map(|index| after + index)
                    .collect();
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
        // Slot 0 is the imported `@define-color`, which now draws from the
        // same counter so a `@media` block's definitions can be spliced back
        // in at their own position (F3); the rules follow it in order.
        assert_eq!(sheet.rules[0].source_order, 1);
        assert_eq!(decl(&sheet.rules[1].declarations, "padding"), "1px");
        assert_eq!(sheet.rules[1].source_order, 2);
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
        assert!(!MediaQuery::Unknown.evaluate(&light));
        // Media Queries 4's third truth value: negating an unknown query
        // still matches nothing (F10).
        assert!(!MediaQuery::Not(std::rc::Rc::new(MediaQuery::Unknown)).evaluate(&light));
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
        assert_eq!(query("(min-width: 100px)"), Some(MediaQuery::Unknown));
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

    /// Skipping a nested `@media` must consume *exactly* its block and stop:
    /// the rules on either side of it, inside the same outer `@media`, still
    /// parse, and so does the top-level rule after the whole thing. The
    /// nested body deliberately contains a `}` and a `"` inside a comment,
    /// and a `}` inside a string, so a skip that scanned for a brace by hand
    /// instead of using the tokenizer's own block tracking would stop in the
    /// wrong place and swallow -- or resurrect -- a sibling.
    #[test]
    fn skipping_a_nested_media_block_stops_exactly_at_its_closing_brace() {
        let css = "@media (prefers-color-scheme: dark) {\n\
                   button { min-width: 41px }\n\
                   @media (prefers-contrast: more) {\n\
                   /* } \" */\n\
                   spinner { min-width: 999px }\n\
                   label { font-family: \"} not a brace {\" }\n\
                   }\n\
                   headerbar { min-height: 42px }\n\
                   }\n\
                   window { min-width: 43px }\n";
        let sheet = parse_stylesheet(css);

        // The rule after the outer `@media` is still top-level.
        let top: Vec<&str> = sheet
            .rules
            .iter()
            .map(|rule| rule.selector_text.as_str())
            .collect();
        assert_eq!(top, ["window"]);

        // Both siblings of the nested block survive, in source order, and
        // nothing from inside the nested block leaked out.
        assert_eq!(sheet.media_blocks.len(), 1);
        let inner: Vec<&str> = sheet.media_blocks[0]
            .rules
            .iter()
            .map(|rule| rule.selector_text.as_str())
            .collect();
        assert_eq!(
            inner,
            ["button", "headerbar"],
            "the nested block's skip did not stop at its own closing brace"
        );
    }

    /// Nested `@media` used to recurse `parse_block -> StyleSheetParser ->
    /// parse_block` with no depth limit of its own, so a hostile `gtk.css`
    /// with a few hundred `@media (...) {` in a row overflowed the stack and
    /// *aborted* the process -- an uncatchable failure, not a parse error.
    /// A nested `@media` block was always dropped whole, so it is now
    /// skipped at the token level instead of being parsed by another
    /// recursive `StyleSheetParser`: the result is the same and the nesting
    /// costs no stack at all.
    #[test]
    fn deeply_nested_media_blocks_are_refused_instead_of_overflowing_the_stack() {
        const DEPTH: usize = 2000;
        let mut css = String::new();
        for _ in 0..DEPTH {
            css.push_str("@media (prefers-color-scheme: dark) {\n");
        }
        css.push_str("button { color: red }\n");
        for _ in 0..DEPTH {
            css.push_str("}\n");
        }
        css.push_str("headerbar { min-height: 33px }\n");

        let sheet = parse_stylesheet(&css);
        // The top-level rule *after* the hostile block is still there, and
        // the outermost `@media` is kept -- carrying nothing, because its
        // one child was a nested `@media`, which is dropped.
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selector_text, "headerbar");
        assert_eq!(sheet.media_blocks.len(), 1);
        assert!(sheet.media_blocks[0].rules.is_empty());
    }
}

#[cfg(test)]
mod review_tests {
    use super::{ColorScheme, Contrast, MediaEnv, MediaQuery, parse_stylesheet};

    fn block(text: &str) -> Option<MediaQuery> {
        let css = format!("@media {text} {{ button {{ color: red }} }}");
        parse_stylesheet(&css)
            .media_blocks
            .first()
            .map(|found| found.query.clone())
    }

    fn applies(text: &str) -> bool {
        block(text).is_some_and(|query| query.evaluate(&MediaEnv::default()))
    }

    // F10: Media Queries 4's three-valued logic. Negating a feature this
    // engine does not model must *not* make the block apply unconditionally.
    #[test]
    fn negating_an_unmodelled_feature_still_matches_nothing() {
        assert_eq!(
            block("not (forced-colors: active)"),
            Some(MediaQuery::Not(std::rc::Rc::new(MediaQuery::Unknown))),
            "the block must still parse"
        );
        assert!(!applies("not (forced-colors: active)"));
        assert!(!applies("(forced-colors: active)"));
        // An unknown operand does not poison a decision the other operand
        // already makes on its own.
        assert!(!applies(
            "(forced-colors: active) and (prefers-color-scheme: dark)"
        ));
        assert!(applies(
            "(forced-colors: active), (prefers-color-scheme: light)"
        ));
        assert!(!applies(
            "(forced-colors: active) and (prefers-color-scheme: light)"
        ));
    }

    // F11: a stray token after a media condition must reject the query, not
    // be swallowed by `expect_comma()` leaving a truncated one behind.
    #[test]
    fn a_media_query_missing_its_and_is_rejected_rather_than_truncated() {
        assert_eq!(
            block("(prefers-color-scheme: dark) (prefers-contrast: more)"),
            None,
            "the malformed query must not be accepted as its first half"
        );
        // The well-formed spelling still works.
        assert!(
            block("(prefers-color-scheme: dark) and (prefers-contrast: more)").is_some_and(
                |query| query.evaluate(&MediaEnv {
                    color_scheme: ColorScheme::Dark,
                    contrast: Contrast::More,
                })
            )
        );
    }

    // F12/F13: the prelude parser is depth-bounded. Both of these aborted the
    // process with a stack overflow before the guard.
    #[test]
    fn a_pathologically_deep_media_prelude_is_refused_rather_than_overflowing() {
        let nots = format!("{}(prefers-color-scheme: dark)", "not ".repeat(30_000));
        assert_eq!(block(&nots), None);
        let parens = format!(
            "{}(prefers-color-scheme: dark){}",
            "(".repeat(15_000),
            ")".repeat(15_000)
        );
        assert_eq!(block(&parens), None);
        // A humanly-deep prelude still parses.
        assert!(applies("((((prefers-color-scheme: light))))"));
    }

    // Priority item: MQ4 media types. An unrecognised type makes the query
    // `not all`; the block still parses instead of being dropped whole.
    #[test]
    fn media_types_and_only_parse_rather_than_dropping_the_block() {
        assert!(applies("screen"));
        assert!(applies("all"));
        assert!(applies("only screen"));
        assert!(applies("screen and (prefers-color-scheme: light)"));
        assert!(!applies("screen and (prefers-color-scheme: dark)"));
        // An unrecognised type parses and never matches; negated, it matches.
        assert_eq!(block("print"), Some(MediaQuery::Always(false)));
        assert!(!applies("print"));
        assert!(applies("not print"));
        assert!(!applies("not screen"));
        // The block itself survives -- that is the whole point.
        let sheet = parse_stylesheet("@media print { button { color: red } }");
        assert_eq!(sheet.media_blocks.len(), 1);
        assert_eq!(sheet.media_blocks[0].rules.len(), 1);
    }

    // F14: the keyframe-selector prelude used the same swallowing
    // `expect_comma()`, so `0% 50% { ... }` was accepted with `50%` dropped.
    #[test]
    fn a_malformed_keyframe_selector_list_is_rejected_not_truncated() {
        let sheet = parse_stylesheet("@keyframes fade { 0% 50% { opacity: 0 } to { opacity: 1 } }");
        let frames = &sheet.keyframes[0].frames;
        assert!(
            frames.iter().all(|(offsets, _)| offsets != &vec![0.0]),
            "the malformed `0% 50%` selector must not survive as a bare 0%"
        );
        assert_eq!(frames.len(), 1, "only the well-formed `to` frame remains");
        assert_eq!(frames[0].0, vec![1.0]);
        // A real comma-separated selector list still parses.
        let ok = parse_stylesheet("@keyframes fade { 0%, 50% { opacity: 0 } }");
        assert_eq!(ok.keyframes[0].frames[0].0, vec![0.0, 0.5]);
    }

    // F20: a function name is re-serialized through `serialize_identifier`,
    // so an escaped name round-trips as the same token rather than as a
    // different function wrapping another.
    #[test]
    fn an_escaped_function_name_round_trips_as_one_token() {
        let sheet = parse_stylesheet("button { background-image: a\\28 b(red) }");
        let value = &sheet.rules[0].declarations[0].value;
        // The round-trip property, not one particular spelling of the escape
        // (`serialize_identifier` writes `\(`, the tokenizer wrote `\28 `):
        // re-tokenizing must give back a *single* function named `a(b`.
        let mut source = cssparser::ParserInput::new(value);
        let mut parser = cssparser::Parser::new(&mut source);
        match parser.next() {
            Ok(cssparser::Token::Function(name)) => {
                assert_eq!(name.as_ref(), "a(b", "re-parsed as a different function");
            }
            other => panic!("expected one function token, got {other:?} from {value:?}"),
        }
        // An ordinary name is untouched.
        let plain = parse_stylesheet("button { background-image: linear-gradient(red, blue) }");
        assert!(
            plain.rules[0].declarations[0]
                .value
                .starts_with("linear-gradient(")
        );
    }
}
