//! Icon theming: freedesktop icon-theme lookup, SVG/PNG rasterisation,
//! GTK's symbolic recolouring, and the `gtkcssimagebuiltin` shapes.
//!
//! Three layers, bottom up:
//!
//! 1. [`theme::IconTheme`] resolves `(name, size, scale, symbolic)` to a file
//!    on disk, following the freedesktop Icon Theme Specification's
//!    `FindIcon` exactly — including its "closest size *within this theme*
//!    before the next theme in the chain" rule, which is the part
//!    implementations usually get wrong.
//! 2. [`render::render`] rasterises one such file at a requested pixel size,
//!    recolouring it first when it is symbolic.
//! 3. `crate::paint::icon::paint_icon` reads the `-gtk-icon-*` properties off
//!    a computed style and draws the result into an allocation.
//!
//! Cursor themes are deliberately absent: icedtea supports
//! `wp_cursor_shape_v1`, so the `cursor` property maps to a protocol shape
//! name and no client-side cursor theme is ever loaded.
//!
//! [`builtin::Builtin`] predates the rest of this module (P5 needed its
//! shapes for `CheckButton`/`SpinButton` before P7 landed); P7 fills in its
//! real geometry in a later task of this part.

pub mod builtin;
pub mod lookup;
pub mod render;
pub mod symbolic;
pub mod theme;

use std::rc::Rc;

// This line grows as items land; see the re-export table in Task 1's
// idempotence note. `Builtin` was already re-exported here by P5.
pub use builtin::Builtin;
pub use theme::{
    DirKind, IconEnv, IconFile, IconTheme, SubDir, ThemeIndex, theme_name_from_settings_ini,
};

/// A rasterised icon, shared by every node that asked for it.
///
/// This is exactly what [`theme::IconTheme::render`] returns and what a
/// widget controller holds on to between frames.
pub type Handle = Rc<skia_rs_safe::codec::Image>;

/// The largest icon this crate will ever rasterise, in device pixels.
///
/// A CSS `-gtk-icon-size: 100000px` is a legal declaration and an
/// `index.theme` may claim `MaxSize=4294967295`; neither may turn into a
/// multi-gigabyte allocation.
pub const MAX_ICON_PX: u32 = 4096;

/// GTK 4's `GtkIconSize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconSize {
    /// Whatever size the context asks for.
    Inherit,
    /// 16 px, GTK's default menu/button icon size.
    Normal,
    /// 32 px.
    Large,
}

impl IconSize {
    /// The pixel size this asks for, given the size inherited from context.
    #[must_use]
    pub const fn pixels(self, inherited: u32) -> u32 {
        match self {
            IconSize::Inherit => inherited,
            IconSize::Normal => 16,
            IconSize::Large => 32,
        }
    }
}

/// The three file formats the icon-theme spec's search knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconFormat {
    /// Scalable vector, the only format symbolic icons come in.
    Svg,
    /// Raster.
    Png,
    /// Looked up for spec fidelity, never decoded: `skia-rs-codec` has no
    /// XPM decoder, so [`render::render`] returns `None` for it and the
    /// caller falls through to `image-missing`.
    Xpm,
}

impl IconFormat {
    /// The format an extension names, ASCII-case-insensitively.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<IconFormat> {
        if ext.eq_ignore_ascii_case("png") {
            Some(IconFormat::Png)
        } else if ext.eq_ignore_ascii_case("svg") {
            Some(IconFormat::Svg)
        } else if ext.eq_ignore_ascii_case("xpm") {
            Some(IconFormat::Xpm)
        } else {
            None
        }
    }

    /// The lower-case extension for this format.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            IconFormat::Svg => "svg",
            IconFormat::Png => "png",
            IconFormat::Xpm => "xpm",
        }
    }
}

/// The extensions the spec searches, in its own order.
///
/// Unused until `lookup::` consumes it in Task 6 — allowed here rather than
/// deferred, since the plan places the constant in this task.
#[allow(dead_code)]
pub(crate) const SEARCH_EXTENSIONS: [IconFormat; 3] =
    [IconFormat::Png, IconFormat::Svg, IconFormat::Xpm];

#[cfg(test)]
pub(crate) mod test_support {
    //! Paths into `ui/tests/fixtures/mini-icon-theme/`.
    //!
    //! Every icon test in this crate is hermetic: it reads this fixture and
    //! never `$XDG_DATA_DIRS`, `/usr/share/icons` or the user's home. The one
    //! exception is the Adwaita probe in `ui/tests/icon_theme.rs`, which is
    //! skipped when the theme is not installed.

    use std::path::PathBuf;

    /// The fixture root, resolved from the crate manifest rather than the
    /// process's working directory (which `cargo test` does not pin).
    pub(crate) fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini-icon-theme")
    }

    /// The two themed roots, in search order.
    pub(crate) fn roots() -> Vec<PathBuf> {
        vec![fixture_dir().join("root-a"), fixture_dir().join("root-b")]
    }

    /// The flat, non-themed last resort.
    pub(crate) fn pixmaps() -> Vec<PathBuf> {
        vec![fixture_dir().join("pixmaps")]
    }
}

#[cfg(test)]
mod mod_tests {
    use super::{IconFormat, IconSize};

    // GTK4's `GtkIconSize`: `Inherit` defers to the caller's size, `Normal`
    // is 16 and `Large` is 32.
    // Mutation check: return `inherited` from the `Large` arm and the third
    // assertion fails.
    #[test]
    fn icon_sizes_are_gtk_fours_sixteen_and_thirty_two() {
        assert_eq!(IconSize::Inherit.pixels(24), 24);
        assert_eq!(IconSize::Normal.pixels(24), 16);
        assert_eq!(IconSize::Large.pixels(24), 32);
    }

    // The spec searches `png`, `svg`, `xpm`, in that order and no other.
    // Mutation check: make `from_extension` case-sensitive and the
    // `"SVG"` assertion fails.
    #[test]
    fn only_the_three_spec_extensions_are_icon_formats() {
        assert_eq!(IconFormat::from_extension("png"), Some(IconFormat::Png));
        assert_eq!(IconFormat::from_extension("svg"), Some(IconFormat::Svg));
        assert_eq!(IconFormat::from_extension("SVG"), Some(IconFormat::Svg));
        assert_eq!(IconFormat::from_extension("xpm"), Some(IconFormat::Xpm));
        assert_eq!(IconFormat::from_extension("jpeg"), None);
        assert_eq!(IconFormat::from_extension(""), None);
        assert_eq!(IconFormat::Png.extension(), "png");
    }

    // The fixture is checked in, not generated at test time: a test that
    // builds its own PNGs proves the builder, not the lookup. This asserts
    // the shape every later task's tests assume.
    // Mutation check: delete `root-b/MiniTheme/16x16/actions/only-in-root-b.png`
    // and this fails -- which is exactly the file that proves an icon is
    // searched across every root, not only the one holding index.theme.
    #[test]
    fn the_mini_icon_theme_fixture_is_complete() {
        use super::test_support::{fixture_dir, pixmaps, roots};

        let root_a = &roots()[0];
        for relative in [
            "MiniTheme/index.theme",
            "MiniTheme/16x16/actions/document-open.png",
            "MiniTheme/16x16/actions/dual.png",
            "MiniTheme/16x16/actions/broken.png",
            "MiniTheme/scalable/actions/document-open.svg",
            "MiniTheme/symbolic/actions/document-open-symbolic.svg",
            "MiniParent/index.theme",
            "MiniParent/24x24/actions/parent-only.png",
            "MiniParent/24x24/actions/dual.png",
            "MiniLoopA/index.theme",
            "MiniLoopB/index.theme",
            "hicolor/index.theme",
            "hicolor/48x48/actions/image-missing.png",
        ] {
            let path = root_a.join(relative);
            assert!(path.is_file(), "missing fixture file {}", path.display());
        }
        assert!(
            roots()[1]
                .join("MiniTheme/16x16/actions/only-in-root-b.png")
                .is_file()
        );
        assert!(pixmaps()[0].join("flat-only.png").is_file());
        assert!(!fixture_dir().join("root-a/MiniTheme/cursors").exists());
    }

    // The PNGs must really decode, and `broken.png` must really not: two
    // later tasks hang their fallback behaviour off that file.
    // Mutation check: replace `broken.png` with a valid PNG and the last
    // assertion fails.
    #[test]
    fn the_fixture_pngs_decode_and_the_broken_one_does_not() {
        use super::test_support::roots;

        let root_a = &roots()[0];
        let good = std::fs::read(root_a.join("MiniTheme/16x16/actions/document-open.png"))
            .expect("fixture readable");
        let image = skia_rs_safe::codec::decode_image(&good).expect("fixture PNG decodes");
        assert_eq!(image.width(), 16);
        assert_eq!(image.height(), 16);

        let missing = std::fs::read(root_a.join("hicolor/48x48/actions/image-missing.png"))
            .expect("fixture readable");
        let image = skia_rs_safe::codec::decode_image(&missing).expect("fixture PNG decodes");
        assert_eq!(image.width(), 48);

        let broken =
            std::fs::read(root_a.join("MiniTheme/16x16/actions/broken.png")).expect("readable");
        assert!(
            skia_rs_safe::codec::decode_image(&broken).is_err(),
            "broken.png decoded, so the decode-failure fallback has nothing to prove"
        );
    }
}
