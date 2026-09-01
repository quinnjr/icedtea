//! GTK's "CSS nodes" notation, both directions.
//!
//! [`render_tree`] prints a retained [`Node`] subtree the way GTK's own
//! documentation prints it; [`matches_fixture`] checks a subtree against a
//! vendored block from those docs. The fixture language has four pieces of
//! slack a concrete tree cannot express -- `[subnode]`, `name[.class]`,
//! `<child>` and `┊` -- so the check is a matcher, not a string compare
//! (contract deviation 8). A fixture with none of them is an exact compare.
//!
//! P5 already ships an equivalent path-based matcher (`fixture_matches`,
//! `node_tree_of`) directly in `widgets::mod`; that one stays untouched so its
//! own tests and `ui/tests/node_trees.rs`'s existing `check` helper keep
//! working unmodified. This module is the richer replacement P6's 27 new
//! fixtures use, via `matches_fixture`'s backtracking pattern matcher and the
//! `<child>`/repetition-aware [`Pattern`] it builds fixtures into.

use std::rc::Rc;

use crate::css::node::Node;

/// Why a subtree did not match its fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The fixture, as given.
    pub expected: String,
    /// The subtree, rendered in the same notation.
    pub rendered: String,
    /// What specifically went wrong, naming the node or class.
    pub reason: Rc<str>,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\n\n--- fixture ---\n{}\n--- rendered ---\n{}",
            self.reason, self.expected, self.rendered
        )
    }
}

/// Render `root` and its descendants in GTK's notation.
#[must_use]
pub fn render_tree(root: &Node) -> String {
    let mut out = String::new();
    out.push_str(&label(root));
    out.push('\n');
    render_children(root, "", &mut out);
    out
}

/// `name.class.class`, classes in the node's own order.
fn label(node: &Node) -> String {
    let mut s = String::from(&*node.name());
    for class in node.classes() {
        s.push('.');
        s.push_str(class.as_str());
    }
    s
}

fn render_children(node: &Node, prefix: &str, out: &mut String) {
    let children = node.children();
    for (i, child) in children.iter().enumerate() {
        let last = i + 1 == children.len();
        out.push_str(prefix);
        out.push_str(if last { "╰── " } else { "├── " });
        out.push_str(&label(child));
        out.push('\n');
        let mut deeper = String::from(prefix);
        deeper.push_str(if last { "    " } else { "│   " });
        render_children(child, &deeper, out);
    }
}

/// One line of a fixture, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    /// `None` for `<child>`, which matches any subtree.
    name: Option<String>,
    /// Classes the node must carry.
    required: Vec<String>,
    /// Classes it may carry; anything else is a failure.
    optional: Vec<String>,
    /// `[subnode]` -- may be absent.
    subnode_optional: bool,
    /// A `┊` line followed this one: it may repeat.
    repeats: bool,
    /// Children, by indentation.
    children: Vec<Pattern>,
}

/// Depth and payload of one fixture line, or `None` for a blank/`┊` line.
fn split_line(line: &str) -> Option<(usize, &str, bool)> {
    let line = line.trim_end();
    let line = line.strip_prefix('\u{feff}').unwrap_or(line);
    if line.trim().is_empty() {
        return None;
    }
    let mut depth = 0usize;
    let mut rest = line;
    loop {
        let next = rest
            .strip_prefix("│   ")
            .or_else(|| rest.strip_prefix("┊   "))
            .or_else(|| rest.strip_prefix("    "));
        match next {
            Some(r) => {
                depth += 1;
                rest = r;
            }
            None => break,
        }
    }
    let (rest, marked) = match rest
        .strip_prefix("├── ")
        .or_else(|| rest.strip_prefix("╰── "))
    {
        Some(r) => (r, true),
        None => (rest, false),
    };
    if marked {
        depth += 1;
    }
    let rest = rest.trim();
    if rest.is_empty() || rest.chars().all(|c| c == '┊' || c == '⋮' || c == '│') {
        // A repetition marker: no node of its own.
        return Some((depth, "", true));
    }
    Some((depth, rest, false))
}

/// `scrollbar.vertical[.dragging]` / `[overshoot.top]` / `<child>`.
fn parse_pattern(text: &str) -> Pattern {
    let mut text = text.trim();
    let mut subnode_optional = false;
    if text.starts_with('[') && text.len() >= 2 {
        // An outer bracket is a subnode marker only when it wraps the *whole*
        // item -- its matching close is the text's last byte. Depth-tracked,
        // not "does the inside contain another `[`": `[scrollbar.vertical
        // [.dragging]]` nests an optional-class bracket *inside* the subnode
        // bracket, and a naive `contains('[')` check would mistake that
        // nesting for two adjacent brackets and skip the outer one.
        let mut depth = 0i32;
        let mut close = None;
        for (i, ch) in text.char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if close == Some(text.len() - 1) {
            subnode_optional = true;
            text = &text[1..text.len() - 1];
        }
    }
    if text.starts_with('<') {
        return Pattern {
            name: None,
            required: Vec::new(),
            optional: Vec::new(),
            subnode_optional: true,
            repeats: false,
            children: Vec::new(),
        };
    }
    let mut name = String::new();
    let mut required = Vec::new();
    let mut optional = Vec::new();
    let mut cursor = text;
    // Leading name, up to the first `.` or `[`.
    let split = cursor.find(['.', '[']).unwrap_or(cursor.len());
    name.push_str(&cursor[..split]);
    cursor = &cursor[split..];
    while !cursor.is_empty() {
        if let Some(rest) = cursor.strip_prefix('[') {
            let end = rest.find(']').unwrap_or(rest.len());
            for class in rest[..end].split('.').filter(|s| !s.is_empty()) {
                optional.push(class.to_string());
            }
            cursor = rest.get(end + 1..).unwrap_or("");
        } else if let Some(rest) = cursor.strip_prefix('.') {
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            if !rest[..end].is_empty() {
                required.push(rest[..end].to_string());
            }
            cursor = &rest[end..];
        } else {
            // Anything else in a fixture line is noise; skip one char and
            // keep going rather than looping forever on it.
            let mut chars = cursor.chars();
            chars.next();
            cursor = chars.as_str();
        }
    }
    Pattern {
        name: Some(name.trim().to_string()),
        required,
        optional,
        subnode_optional,
        repeats: false,
        children: Vec::new(),
    }
}

/// Parse a whole fixture into its root pattern.
fn parse_fixture(text: &str) -> Option<Pattern> {
    let mut stack: Vec<Pattern> = Vec::new();
    let mut root: Option<Pattern> = None;
    for line in text.lines() {
        let Some((depth, payload, is_repeat)) = split_line(line) else {
            continue;
        };
        if is_repeat {
            // Mark the deepest pattern parsed so far as repeatable.
            if let Some(last) = stack.last_mut().and_then(|p| p.children.last_mut()) {
                last.repeats = true;
            } else if let Some(last) = stack.last_mut() {
                last.repeats = true;
            }
            continue;
        }
        let pattern = parse_pattern(payload);
        if depth == 0 {
            if root.is_some() {
                // A second root line: fixtures with two alternatives (GTK's
                // `separator.horizontal` / `separator.vertical`) are matched
                // by whichever alternative the caller's tree is.
                continue;
            }
            stack.clear();
            stack.push(pattern);
            continue;
        }
        while stack.len() > depth {
            let done = stack.pop()?;
            stack.last_mut()?.children.push(done);
        }
        if stack.len() != depth {
            // Malformed indentation: attach at the current level rather than
            // indexing past the stack.
            stack.last_mut()?.children.push(pattern);
            continue;
        }
        stack.push(pattern);
    }
    while stack.len() > 1 {
        let done = stack.pop()?;
        stack.last_mut()?.children.push(done);
    }
    root = stack.pop().or(root);
    if let Some(pat) = root.as_mut() {
        coalesce_repeats(pat);
    }
    root
}

/// Fold a `┊`-marked pattern back into an identical sibling that follows it.
///
/// GTK's docs draw a repeated run as *first line*, `┊`, *last line* -- the
/// last line is the tree-drawing "closing" sibling, not a second, distinct,
/// unconditionally-required node. [`split_line`] attaches the marker to
/// whichever pattern is on top of the stack when it is seen (the first
/// line), so without this pass an empty run (one child, matching only the
/// first line) is rejected: the parsed pattern list would still demand the
/// closing line's node as a mandatory extra sibling.
fn coalesce_repeats(pattern: &mut Pattern) {
    let mut i = 0;
    while i + 1 < pattern.children.len() {
        let same = pattern.children[i].repeats
            && pattern.children[i].name == pattern.children[i + 1].name
            && pattern.children[i].required == pattern.children[i + 1].required
            && pattern.children[i].optional == pattern.children[i + 1].optional
            && pattern.children[i].subnode_optional == pattern.children[i + 1].subnode_optional
            && pattern.children[i].children == pattern.children[i + 1].children;
        if same {
            pattern.children.remove(i + 1);
        } else {
            i += 1;
        }
    }
    for child in &mut pattern.children {
        coalesce_repeats(child);
    }
}

/// Does `node` satisfy `pattern`'s own name and classes?
fn head_matches(pattern: &Pattern, node: &Node) -> Result<(), String> {
    let Some(expected) = pattern.name.as_deref() else {
        return Ok(());
    };
    if &*node.name() != expected {
        return Err(format!(
            "expected node `{expected}`, found `{}`",
            node.name()
        ));
    }
    let classes: Vec<String> = node
        .classes()
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    for want in &pattern.required {
        if !classes.iter().any(|c| c == want) {
            return Err(format!(
                "node `{expected}` is missing the required class `{want}` (has {classes:?})"
            ));
        }
    }
    Ok(())
}

/// Backtracking match of a pattern list against a child list.
fn match_list(pats: &[Pattern], actual: &[Node], reason: &mut String) -> bool {
    fn go(pats: &[Pattern], actual: &[Node], pi: usize, ai: usize, reason: &mut String) -> bool {
        if pi == pats.len() {
            if ai == actual.len() {
                return true;
            }
            *reason = format!(
                "unexpected extra subnode `{}`",
                actual
                    .get(ai)
                    .map_or_else(String::new, |n| n.name().to_string())
            );
            return false;
        }
        let pat = &pats[pi];
        let optional = pat.subnode_optional || pat.name.is_none();
        let max_repeat = if pat.repeats || pat.name.is_none() {
            actual.len() - ai
        } else {
            1
        };
        let min_repeat = usize::from(!optional);
        // Greedy first, then fewer: `<child>` and `┊` both want to absorb.
        for take in (min_repeat..=max_repeat).rev() {
            let mut ok = true;
            for k in 0..take {
                let Some(node) = actual.get(ai + k) else {
                    ok = false;
                    break;
                };
                if let Err(why) = head_matches(pat, node) {
                    *reason = why;
                    ok = false;
                    break;
                }
                if !pat.children.is_empty() && !match_list(&pat.children, &node.children(), reason)
                {
                    ok = false;
                    break;
                }
            }
            if ok && go(pats, actual, pi + 1, ai + take, reason) {
                return true;
            }
        }
        if optional && go(pats, actual, pi + 1, ai, reason) {
            return true;
        }
        if reason.is_empty() {
            *reason = format!(
                "no arrangement of the children matches `{}`",
                pat.name.as_deref().unwrap_or("<child>")
            );
        }
        false
    }
    let mut r = String::new();
    let ok = go(pats, actual, 0, 0, &mut r);
    if !ok && !r.is_empty() {
        *reason = r;
    }
    ok
}

/// Check `root` against a vendored GTK "CSS nodes" block.
///
/// # Errors
///
/// [`Mismatch`] naming the first node, class or arrangement that differs.
pub fn matches_fixture(root: &Node, fixture: &str) -> Result<(), Mismatch> {
    let rendered = render_tree(root);
    let fail = |reason: String| Mismatch {
        expected: fixture.to_string(),
        rendered: rendered.clone(),
        reason: Rc::from(reason.as_str()),
    };
    let Some(pattern) = parse_fixture(fixture) else {
        return Err(fail("the fixture parsed to nothing".to_string()));
    };
    head_matches(&pattern, root).map_err(fail)?;
    let mut reason = String::new();
    if match_list(&pattern.children, &root.children(), &mut reason) {
        Ok(())
    } else {
        Err(fail(reason))
    }
}

#[cfg(test)]
mod tests {
    use super::{matches_fixture, render_tree};
    use crate::css::node::Node;

    /// `paned > (widget, separator.wide, widget)`.
    fn paned() -> Node {
        let root = Node::new("paned");
        let a = Node::new("widget");
        let sep = Node::with_classes("separator", &["wide"]);
        let b = Node::new("widget");
        for n in [&a, &sep, &b] {
            root.append_child(n);
        }
        root
    }

    #[test]
    fn a_tree_renders_in_gtks_own_notation() {
        // Mutation check: emitting "|--" instead of GTK's "├──", or losing
        // the leading class dot, makes this string compare fail and every
        // fixture unreadable next to the docs it was copied from.
        assert_eq!(
            render_tree(&paned()),
            "paned\n├── widget\n├── separator.wide\n╰── widget\n"
        );
    }

    #[test]
    fn an_exact_fixture_matches_and_a_wrong_name_does_not() {
        // Mutation check: matching on the first child only (forgetting to
        // advance the actual cursor) accepts the second fixture too.
        assert!(
            matches_fixture(
                &paned(),
                "paned\n├── <child>\n├── separator[.wide]\n╰── <child>\n"
            )
            .is_ok()
        );
        let err = matches_fixture(&paned(), "paned\n├── <child>\n├── handle\n╰── <child>\n")
            .expect_err("a renamed subnode must fail");
        assert!(err.reason.contains("handle"), "the reason names the node");
        assert!(
            err.rendered.contains("separator.wide"),
            "the render is attached"
        );
    }

    #[test]
    fn optional_subnodes_may_be_absent_and_optional_classes_may_be_missing() {
        // Mutation check: treating `[name]` as required fails the first
        // assertion; treating `name[.class]` as requiring the class fails
        // the second.
        let sw = Node::new("scrolledwindow");
        let child = Node::new("widget");
        let bar = Node::with_classes("scrollbar", &["vertical"]);
        sw.append_child(&child);
        sw.append_child(&bar);
        let fixture = "scrolledwindow[.frame]\n├── <child>\n├── [overshoot.top]\n\
                       ├── [scrollbar.horizontal]\n╰── [scrollbar.vertical[.dragging]]\n";
        assert!(matches_fixture(&sw, fixture).is_ok());
        assert!(matches_fixture(&Node::new("scrolledwindow"), "scrolledwindow[.frame]\n").is_ok());
    }

    #[test]
    fn a_repetition_marker_accepts_any_number_of_the_line_above() {
        // Mutation check: consuming exactly one repeat makes the three-item
        // menu fail; consuming zero makes the empty one fail.
        let menu = Node::with_classes("popover", &["background", "menu"]);
        for _ in 0..3 {
            menu.append_child(&Node::with_classes("button", &["model"]));
        }
        let fixture = "popover.background.menu\n├── button.model\n┊\n╰── button.model\n";
        assert!(matches_fixture(&menu, fixture).is_ok());
        let empty = Node::with_classes("popover", &["background", "menu"]);
        empty.append_child(&Node::with_classes("button", &["model"]));
        assert!(matches_fixture(&empty, fixture).is_ok());
    }

    #[test]
    fn a_required_class_that_is_missing_fails_with_a_reason() {
        // Mutation check: comparing only node names lets a `list` without
        // `.navigation-sidebar` pass, and the sidebar's whole styling with it.
        let list = Node::new("list");
        let err = matches_fixture(&list, "list.navigation-sidebar\n").expect_err("missing class");
        assert!(err.reason.contains("navigation-sidebar"));
    }

    #[test]
    fn a_hostile_fixture_never_panics() {
        // Fixtures are vendored, but they are text: a bad rebase, a stray
        // BOM or a truncated copy must fail the test, never abort the run.
        for text in [
            "",
            "\u{feff}box",
            "├──",
            "╰── ╰── ╰──",
            "[",
            "[[[[[[[[[[",
            "box\n\t\t\t\tmangled",
            "name.\n",
            "\u{0}\u{1}\u{2}",
            &"├── x\n".repeat(5_000),
            &format!("box\n{}", "    ".repeat(4_000)),
        ] {
            let _ = matches_fixture(&paned(), text);
        }
    }
}
