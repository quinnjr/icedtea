//! The freedesktop Icon Theme Specification's `FindIcon` search.
//!
//! The shape that matters, and that a naive implementation loses: the
//! closest-size fallback runs **per theme**, before the next theme in the
//! inheritance chain. A theme that has the icon at the wrong size beats a
//! parent that has it at the right one — otherwise a partial icon theme
//! would silently mix its own art with its parent's at every size it happens
//! not to ship.

use std::path::{Path, PathBuf};

use super::theme::{IconFile, IconTheme, SubDir, ThemeIndex};
use super::{IconFormat, MAX_ICON_PX, SEARCH_EXTENSIONS};

/// The icon every miss falls back to.
///
/// Unused outside this module's own tests until a later task's cache wires
/// `lookup_uncached` in — allowed here rather than deferred, since the plan
/// places these items in this task.
#[allow(dead_code)]
pub(crate) const MISSING_ICON: &str = "image-missing";

/// The longest icon name this will look for.
#[allow(dead_code)]
const MAX_ICON_NAME: usize = 255;

/// `true` if `name` can be a file stem: non-empty, no separators, no `.`
/// components, no NUL, and short enough to be a filename.
///
/// Icon names arrive from CSS (`-gtk-icontheme(name)`) and from application
/// code; joining `../../etc/passwd` onto a theme directory would walk out of
/// the search roots.
#[allow(dead_code)]
pub(crate) fn is_safe_icon_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ICON_NAME
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// `true` for a `-symbolic` file stem, or any path with a `symbolic`
/// directory component.
#[allow(dead_code)]
pub(crate) fn path_is_symbolic(path: &Path) -> bool {
    let stem_symbolic = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("-symbolic"));
    let dir_symbolic = path
        .components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case("symbolic"));
    stem_symbolic || dir_symbolic
}

/// The first existing `<root>/<theme>/<subdir>/<name>.<ext>`, in the spec's
/// extension order and the roots' own order.
#[allow(dead_code)]
fn first_existing(
    theme: &IconTheme,
    theme_name: &str,
    subdir: &SubDir,
    name: &str,
) -> Option<(PathBuf, IconFormat)> {
    for format in SEARCH_EXTENSIONS {
        let file = format!("{name}.{}", format.extension());
        for path in theme.candidate_paths(theme_name, &subdir.path, &file) {
            if path.is_file() {
                return Some((path, format));
            }
        }
    }
    None
}

/// Build the `IconFile` for a hit in `subdir`.
#[allow(dead_code)]
fn icon_file(subdir: &SubDir, path: PathBuf, format: IconFormat) -> IconFile {
    IconFile {
        symbolic: path_is_symbolic(&path),
        path,
        format,
        nominal_size: subdir.size,
        scale: subdir.scale,
        kind: subdir.kind,
    }
}

/// The spec's `FindIconHelper` for one theme: the exact-match pass, then that
/// same theme's minimum-distance candidate.
#[allow(dead_code)]
fn find_in_theme(
    theme: &IconTheme,
    index: &ThemeIndex,
    theme_name: &str,
    name: &str,
    size: u32,
    scale: u32,
) -> Option<IconFile> {
    for subdir in &index.dirs {
        if !subdir.kind.matches(subdir.scale, size, scale) {
            continue;
        }
        if let Some((path, format)) = first_existing(theme, theme_name, subdir, name) {
            return Some(icon_file(subdir, path, format));
        }
    }

    let mut best: Option<(u32, IconFile)> = None;
    for subdir in &index.dirs {
        let Some((path, format)) = first_existing(theme, theme_name, subdir, name) else {
            continue;
        };
        let distance = subdir.kind.distance(subdir.scale, size, scale);
        let candidate = icon_file(subdir, path, format);
        match &best {
            Some((best_distance, _)) if *best_distance <= distance => {}
            _ => best = Some((distance, candidate)),
        }
    }
    best.map(|(_, file)| file)
}

/// `<pixmaps>/<name>.<ext>`, the flat non-themed last resort.
#[allow(dead_code)]
fn find_pixmap(theme: &IconTheme, name: &str, size: u32) -> Option<IconFile> {
    for format in SEARCH_EXTENSIONS {
        let file = format!("{name}.{}", format.extension());
        for dir in theme.pixmap_dirs() {
            let path = dir.join(&file);
            if path.is_file() {
                return Some(IconFile {
                    symbolic: path_is_symbolic(&path),
                    path,
                    format,
                    nominal_size: size,
                    scale: 1,
                    kind: super::DirKind::Fixed { size },
                });
            }
        }
    }
    None
}

/// One name, searched through the whole chain and then the pixmaps.
#[allow(dead_code)]
fn find_one(theme: &IconTheme, name: &str, size: u32, scale: u32) -> Option<IconFile> {
    if !is_safe_icon_name(name) {
        return None;
    }
    for theme_name in theme.chain() {
        let Some(index) = theme.index(theme_name) else {
            continue;
        };
        if let Some(file) = find_in_theme(theme, index, theme_name, name, size, scale) {
            return Some(file);
        }
    }
    find_pixmap(theme, name, size)
}

/// The spec's `FindIcon`, plus GTK's symbolic preference and the
/// `image-missing` backstop.
///
/// `size`/`scale` are clamped into `1..=MAX_ICON_PX` / `1..=4`: both come
/// from CSS, where `-gtk-icon-size: 0` and `-gtk-icon-size: 1e9px` are legal
/// declarations.
///
/// `None` means the icon, *and* `image-missing`, are absent everywhere — the
/// caller draws nothing rather than a made-up path.
#[allow(dead_code)]
pub(crate) fn lookup_uncached(
    theme: &IconTheme,
    name: &str,
    size: u32,
    scale: u32,
    symbolic: bool,
) -> Option<IconFile> {
    let size = size.clamp(1, MAX_ICON_PX);
    let scale = scale.clamp(1, 4);

    let mut names: Vec<String> = Vec::with_capacity(4);
    if symbolic && !name.ends_with("-symbolic") {
        names.push(format!("{name}-symbolic"));
    }
    names.push(name.to_owned());
    if name != MISSING_ICON {
        if symbolic {
            names.push(format!("{MISSING_ICON}-symbolic"));
        }
        names.push(MISSING_ICON.to_owned());
    }

    for candidate in &names {
        if let Some(file) = find_one(theme, candidate, size, scale) {
            return Some(file);
        }
    }
    tracing::debug!(
        icon = name,
        theme = theme.name(),
        "icon not found in any theme in the chain, and neither is image-missing"
    );
    None
}

#[cfg(test)]
mod tests {
    use super::{MISSING_ICON, is_safe_icon_name, lookup_uncached};
    use crate::icons::test_support::{pixmaps, roots};
    use crate::icons::{DirKind, IconFormat, IconTheme};

    fn mini() -> IconTheme {
        IconTheme::with_name_and_roots("MiniTheme", roots())
    }

    fn name_of(path: &std::path::Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    // The plain case: a Fixed 16 directory holding a PNG, asked for at 16.
    // Mutation check: search the extensions as svg-before-png and this
    // returns nothing (16x16/actions has no SVG at all).
    #[test]
    fn an_exact_size_match_wins() {
        let theme = mini();
        let file = lookup_uncached(&theme, "document-open", 16, 1, false).expect("found");
        assert_eq!(name_of(&file.path), "document-open.png");
        assert_eq!(file.format, IconFormat::Png);
        assert_eq!(file.nominal_size, 16);
        assert_eq!(file.scale, 1);
        assert!(!file.symbolic);
        assert_eq!(file.kind, DirKind::Fixed { size: 16 });
        assert!(
            file.path
                .ends_with("root-a/MiniTheme/16x16/actions/document-open.png")
        );
    }

    // A Scalable directory matches its whole range, so the same name at 128
    // resolves to the SVG instead.
    // Mutation check: make `matches` ignore Scalable's range and this
    // returns the 16px PNG.
    #[test]
    fn a_scalable_directory_serves_a_size_the_fixed_one_cannot() {
        let theme = mini();
        let file = lookup_uncached(&theme, "document-open", 128, 1, false).expect("found");
        assert_eq!(name_of(&file.path), "document-open.svg");
        assert_eq!(file.format, IconFormat::Svg);
    }

    // No directory matches 4px (Fixed is 16, Scalable starts at 8), so the
    // closest-size pass runs: Scalable's distance is 4, Fixed's is 12.
    // Mutation check: return the *first* candidate instead of the minimum
    // distance one and this returns the PNG.
    #[test]
    fn with_no_exact_match_the_closest_size_in_that_theme_wins() {
        let theme = mini();
        let file = lookup_uncached(&theme, "document-open", 4, 1, false).expect("found");
        assert_eq!(name_of(&file.path), "document-open.svg");
    }

    // The subtle rule, and the one implementations usually get wrong: a
    // theme that has the icon at the wrong size still beats a parent that
    // has it at exactly the right one. `dual` is 16px in MiniTheme and 24px
    // in MiniParent; asked for at 24, MiniTheme's 16px file wins.
    // Mutation check: run the closest-size pass across the whole chain
    // instead of per theme and MiniParent's exact 24px file wins.
    #[test]
    fn a_wrong_size_icon_in_this_theme_beats_a_right_size_one_in_a_parent() {
        let theme = mini();
        let file = lookup_uncached(&theme, "dual", 24, 1, false).expect("found");
        assert!(
            file.path
                .ends_with("root-a/MiniTheme/16x16/actions/dual.png"),
            "resolved to {}",
            file.path.display()
        );
    }

    // Inheritance itself: an icon only the parent has.
    // Mutation check: stop walking past the first theme and this returns
    // image-missing.
    #[test]
    fn an_icon_only_the_parent_theme_has_is_found_through_inherits() {
        let theme = mini();
        let file = lookup_uncached(&theme, "parent-only", 24, 1, false).expect("found");
        assert!(
            file.path
                .ends_with("root-a/MiniParent/24x24/actions/parent-only.png")
        );
        assert_eq!(
            file.kind,
            DirKind::Threshold {
                size: 24,
                threshold: 2
            }
        );
    }

    // index.theme is read from the first root that has one, but icon *files*
    // are searched across every root -- which is how a user's
    // ~/.local/share/icons overrides one file of a system theme.
    // Mutation check: search only the root that supplied index.theme and
    // this returns image-missing.
    #[test]
    fn an_icon_file_is_searched_across_every_root() {
        let theme = mini();
        let file = lookup_uncached(&theme, "only-in-root-b", 16, 1, false).expect("found");
        assert!(
            file.path
                .ends_with("root-b/MiniTheme/16x16/actions/only-in-root-b.png")
        );
    }

    // A symbolic request prefers `<name>-symbolic` ...
    // Mutation check: drop the `-symbolic` name and this returns the 16px
    // full-colour PNG, whose recolour would then be a no-op.
    #[test]
    fn a_symbolic_request_prefers_the_symbolic_variant() {
        let theme = mini();
        let file = lookup_uncached(&theme, "document-open", 16, 1, true).expect("found");
        assert_eq!(name_of(&file.path), "document-open-symbolic.svg");
        assert!(file.symbolic);
    }

    // ... and falls back to the plain name when there is no symbolic variant.
    // Mutation check: stop appending the plain name and this returns
    // image-missing.
    #[test]
    fn a_symbolic_request_falls_back_to_the_regular_icon() {
        let theme = mini();
        let file = lookup_uncached(&theme, "dual", 16, 1, true).expect("found");
        assert_eq!(name_of(&file.path), "dual.png");
        assert!(!file.symbolic);
    }

    // A total miss lands on image-missing, which only hicolor has -- so this
    // also proves the implicit hicolor tail of the chain.
    // Mutation check: return None instead of retrying image-missing and this
    // fails, and every widget with a bad icon name paints nothing at all.
    #[test]
    fn a_total_miss_falls_back_to_image_missing() {
        let theme = mini();
        let file = lookup_uncached(&theme, "no-such-icon", 48, 1, false).expect("found");
        assert_eq!(name_of(&file.path), "image-missing.png");
        assert!(
            file.path
                .ends_with("root-a/hicolor/48x48/actions/image-missing.png")
        );
    }

    // And when even image-missing is absent, `None` -- never a panic and
    // never a made-up path.
    // Mutation check: return a synthesised path and `is_none` fails.
    #[test]
    fn with_no_roots_at_all_everything_misses() {
        let theme = IconTheme::with_name_and_roots("MiniTheme", Vec::new());
        assert!(lookup_uncached(&theme, "document-open", 16, 1, false).is_none());
        assert!(lookup_uncached(&theme, MISSING_ICON, 16, 1, false).is_none());
    }

    // The flat /usr/share/pixmaps last resort, exercised hermetically.
    // Mutation check: search pixmaps before the themes and
    // `an_exact_size_match_wins` still passes but this file's nominal_size
    // assertion changes -- so the ordering is pinned by the assertion that
    // `document-open` still comes from the theme.
    #[test]
    fn a_flat_pixmap_is_the_last_resort_after_every_theme() {
        let theme = IconTheme::with_name_roots_and_pixmaps("MiniTheme", roots(), pixmaps());
        let file = lookup_uncached(&theme, "flat-only", 16, 1, false).expect("found");
        assert!(file.path.ends_with("pixmaps/flat-only.png"));
        assert_eq!(file.nominal_size, 16);
        assert_eq!(file.kind, DirKind::Fixed { size: 16 });

        let themed = lookup_uncached(&theme, "document-open", 16, 1, false).expect("found");
        assert!(
            themed
                .path
                .ends_with("root-a/MiniTheme/16x16/actions/document-open.png")
        );
    }

    // Icon names come from CSS and from application code; sizes and scales
    // come from CSS too. None of them may panic or escape the roots.
    // Mutation check: drop `is_safe_icon_name` and the "../.." case resolves
    // to a path outside the fixture.
    #[test]
    fn hostile_names_sizes_and_scales_never_panic_and_never_escape() {
        let theme = IconTheme::with_name_roots_and_pixmaps("MiniTheme", roots(), pixmaps());
        for name in [
            "",
            "/etc/passwd",
            "../../../etc/passwd",
            "..",
            ".",
            "a/b",
            "a\\b",
            "a\0b",
            "document-open.png",
            &"x".repeat(4096),
        ] {
            let found = lookup_uncached(&theme, name, 16, 1, false);
            if let Some(file) = found {
                assert!(
                    file.path.ends_with("image-missing.png"),
                    "{name:?} resolved to {}",
                    file.path.display()
                );
            }
        }
        assert!(!is_safe_icon_name("a/b"));
        assert!(is_safe_icon_name("document-open"));

        for (size, scale) in [
            (0, 0),
            (0, 1),
            (1, 0),
            (u32::MAX, u32::MAX),
            (u32::MAX, 1),
            (1, u32::MAX),
        ] {
            let _ = lookup_uncached(&theme, "document-open", size, scale, false);
            let _ = lookup_uncached(&theme, "document-open", size, scale, true);
        }
    }
}
