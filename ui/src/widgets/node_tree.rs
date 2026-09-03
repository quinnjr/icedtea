//! GTK's "CSS nodes" notation, both directions.
//!
//! [`render_node_tree`] prints a retained [`Node`] subtree the way GTK's own
//! documentation prints it; [`fixture_matches`] checks a rendered subtree
//! against a vendored block from those docs, and [`matches_fixture`] is the
//! node-first spelling of the same check. The fixture language has four
//! pieces of slack a concrete tree cannot express -- `[subnode]`,
//! `name[.class]`, `<child>` and `┊` -- so the check is a matcher, not a
//! string compare (contract deviation 8). A fixture with none of them
//! compares exactly.
//!
//! **One matcher, per contract §11 E6.** P5 shipped `node_tree_of`,
//! `render_node_tree` and `fixture_matches` in `widgets::mod`; this module
//! is where E6 relocates them, unchanged in name and signature, and
//! `matches_fixture(root, fixture)` is the thin node-first wrapper E6
//! specifies -- it renders `root` and calls `fixture_matches`, boxing the
//! `String` reason into a [`Mismatch`] that also carries the render. P6's
//! own second implementation (a backtracking `Pattern` matcher) is gone:
//! two matchers with different slack semantics meant P5's 32 fixtures and
//! P6's 27 were validated against different rules, which is exactly what
//! E6 forbids.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::{Kind, Props};

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

/// Check `root` against a vendored GTK "CSS nodes" block.
///
/// The node-first spelling of [`fixture_matches`], and nothing more: it
/// renders `root` with [`render_node_tree`] and delegates. There is exactly
/// one matcher implementation (contract §11 E6).
///
/// # Errors
///
/// [`Mismatch`] naming the first node, class or arrangement that differs.
pub fn matches_fixture(root: &Node, fixture: &str) -> Result<(), Mismatch> {
    let rendered = render_node_tree(root);
    fixture_matches(fixture, &rendered).map_err(|reason| Mismatch {
        expected: fixture.to_string(),
        rendered,
        reason: Rc::from(reason.as_str()),
    })
}

/// Build `kind` in isolation and render its retained subtree in GTK notation.
///
/// Hermetic: no compositor and no window. The build goes through
/// [`build_widget`](crate::widgets::build_widget), so this path and every
/// widget module's own `matches_fixture` test see the *same*
/// [`Headless`](crate::widgets::Headless) build environment -- the bundled
/// Adwaita light sheet, not the empty sheet this function used to compile
/// for itself. Two build environments diverge the moment a controller
/// branches on computed style.
#[must_use]
pub fn node_tree_of(kind: Kind, props: &Props) -> String {
    render_node_tree(&crate::widgets::build_widget::<()>(kind, props).node)
}

/// Render a retained subtree in GTK's own `├──`/`╰──` notation.
///
/// `name.class1.class2` per node, classes in the order the node reports them.
#[must_use]
pub fn render_node_tree(root: &Node) -> String {
    fn spec(node: &Node) -> String {
        let mut out = node.name().to_string();
        for class in node.classes() {
            out.push('.');
            out.push_str(class.as_str());
        }
        out
    }
    fn walk(node: &Node, prefix: &str, out: &mut String) {
        let children = node.children();
        for (index, child) in children.iter().enumerate() {
            let last = index + 1 == children.len();
            out.push_str(prefix);
            out.push_str(if last { "╰── " } else { "├── " });
            out.push_str(&spec(child));
            out.push('\n');
            let mut next = prefix.to_owned();
            next.push_str(if last { "    " } else { "│   " });
            walk(child, &next, out);
        }
    }
    let mut out = spec(root);
    out.push('\n');
    walk(root, "", &mut out);
    out
}

/// One parsed line of either tree.
struct TreeLine {
    depth: usize,
    name: String,
    /// Classes the node must carry.
    required: Vec<String>,
    /// The whole node is configuration-dependent (`[name]`).
    node_optional: bool,
    /// `<child>`: an arbitrary application subtree.
    wildcard: bool,
}

/// Parse a GTK node-tree block into lines with their depths.
///
/// Repetition markers (`┊`, `⋮`) and blank lines are dropped: repetition does
/// not change which *paths* are legal, which is what the matcher checks.
fn parse_tree(text: &str) -> Vec<TreeLine> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() || line.trim().starts_with("```") {
            continue;
        }
        let trimmed = line.trim_start_matches(['│', '┊', '⋮', ' ']);
        if trimmed.is_empty() {
            continue;
        }
        // Depth is one level per four columns of prefix.
        let prefix_cols = line.chars().count() - trimmed.chars().count();
        let (depth, body) = match trimmed
            .strip_prefix("├── ")
            .or_else(|| trimmed.strip_prefix("╰── "))
        {
            Some(body) => (prefix_cols / 4 + 1, body),
            None => (0, trimmed),
        };
        let body = body.trim();
        // Any line opening with a `<...>` token is an application-supplied
        // subtree GTK's block does not describe: `<child>`, but also
        // `<overlay child>[.left][.right]` and `<column header>`, which the
        // vendored blocks use verbatim. Whatever follows the token (class
        // markers a *specific* application child would carry) is slack for
        // the same reason the subtree is.
        if body.starts_with('<') && body.contains('>') {
            out.push(TreeLine {
                depth,
                name: String::new(),
                required: Vec::new(),
                node_optional: false,
                wildcard: true,
            });
            continue;
        }
        let (body, node_optional) = match body.strip_prefix('[') {
            Some(rest) => (rest.trim_end_matches(']'), true),
            None => (body, false),
        };
        let mut required = Vec::new();
        // The name runs up to the first '.' or '['.
        let head_end = body.find(['.', '[']).unwrap_or(body.len());
        let name = body[..head_end].to_owned();
        let mut rest = &body[head_end..];
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                // An `[.class]` is optional: it is neither required of the
                // rendered node nor forbidden on it.
                let end = after.find(']').unwrap_or(after.len());
                rest = &after[(end + 1).min(after.len())..];
            } else if let Some(after) = rest.strip_prefix('.') {
                let end = after.find(['.', '[']).unwrap_or(after.len());
                required.push(after[..end].to_owned());
                rest = &after[end..];
            } else {
                break;
            }
        }
        out.push(TreeLine {
            depth,
            name,
            required,
            node_optional,
            wildcard: false,
        });
    }
    out
}

/// Turn parsed lines into `(path, line index)` pairs, where a path is the
/// slash-joined node names from the root.
fn paths(lines: &[TreeLine]) -> Vec<(String, usize)> {
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        stack.truncate(line.depth);
        let name = if line.wildcard {
            "<child>".to_owned()
        } else {
            line.name.clone()
        };
        stack.push(name);
        out.push((stack.join("/"), index));
    }
    out
}

/// Whether fixture line `index` has at least one child line.
fn has_children(lines: &[TreeLine], index: usize) -> bool {
    let depth = lines[index].depth;
    lines[index + 1..]
        .iter()
        .take_while(|line| line.depth > depth)
        .any(|line| line.depth == depth + 1)
}

/// Match a rendered tree against a vendored GTK fixture.
///
/// Both directions are enforced. Every rendered node must sit at a path the
/// fixture names (or under a `<child>` wildcard), and must carry every class
/// the fixture marks as always-present. Every fixture node that is not
/// `[optional]` and whose parent is present must appear in the rendered tree.
/// Extra classes on a rendered node are allowed: GTK's blocks do not list
/// application-supplied classes such as `.suggested-action`.
///
/// # Errors
///
/// A human-readable description of the first mismatch.
pub fn fixture_matches(fixture: &str, rendered: &str) -> Result<(), String> {
    let fixture_lines = parse_tree(fixture);
    let rendered_lines = parse_tree(rendered);
    let fixture_paths = paths(&fixture_lines);
    let rendered_paths = paths(&rendered_lines);

    let wildcard_prefixes: Vec<String> = fixture_paths
        .iter()
        .filter(|(_, index)| fixture_lines[*index].wildcard)
        .map(|(path, _)| path.trim_end_matches("/<child>").to_owned())
        .collect();

    // A fixture line with no children of its own describes a node whose
    // subtree the vendored block elides -- GTK's `windowcontrols`, a menu
    // `popover`, a `columnview`'s column header. Its rendered descendants
    // are outside what the fixture claims, so they are not checked; only
    // the node itself is.
    let elided_prefixes: Vec<&str> = fixture_paths
        .iter()
        .filter(|(_, index)| !has_children(&fixture_lines, *index))
        .map(|(path, _)| path.as_str())
        .collect();

    for (path, index) in &rendered_paths {
        if wildcard_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix) && path.len() > prefix.len())
        {
            continue;
        }
        if elided_prefixes.iter().any(|prefix| {
            path.len() > prefix.len()
                && path.starts_with(prefix)
                && path[prefix.len()..].starts_with('/')
        }) {
            continue;
        }
        let candidates: Vec<usize> = fixture_paths
            .iter()
            .filter(|(fixture_path, _)| fixture_path == path)
            .map(|(_, i)| *i)
            .collect();
        if candidates.is_empty() {
            return Err(format!("rendered node at '{path}' is not in the fixture"));
        }
        // Two or more fixture lines can share a path — e.g. `block.filled`
        // and `block.empty` at `levelbar/trough/block` — to describe
        // mutually exclusive variants a repeated child can take. The
        // rendered node only needs to satisfy one of them.
        let actual = &rendered_lines[*index];
        let mut missing: Option<&str> = None;
        let matched = candidates.iter().any(|fixture_index| {
            let expected = &fixture_lines[*fixture_index];
            match expected
                .required
                .iter()
                .find(|c| !actual.required.contains(*c))
            {
                Some(class) => {
                    missing.get_or_insert(class.as_str());
                    false
                }
                None => true,
            }
        });
        if !matched {
            let class = missing.unwrap_or("");
            return Err(format!(
                "required class '{class}' missing from rendered node at '{path}'"
            ));
        }
    }

    let rendered_set: std::collections::HashSet<&str> = rendered_paths
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    for (path, index) in &fixture_paths {
        let line = &fixture_lines[*index];
        if line.node_optional || line.wildcard {
            continue;
        }
        let parent = match path.rfind('/') {
            Some(cut) => &path[..cut],
            None => "",
        };
        if !parent.is_empty() && !rendered_set.contains(parent) {
            continue;
        }
        if !rendered_set.contains(path.as_str()) {
            return Err(format!(
                "fixture requires a node at '{path}', which was not rendered"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{matches_fixture, render_node_tree};
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
            render_node_tree(&paned()),
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
