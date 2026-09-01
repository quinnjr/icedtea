//! The `gallery` binary, as library code.
//!
//! Lives in the library rather than in `src/bin/gallery.rs` because [`Kind`]
//! is `#[non_exhaustive]`: an exhaustive `match` over it compiles only inside
//! the crate that defines it, and "a `Kind` with no gallery entry does not
//! compile" is the completeness guarantee the M3 gate rests on.

use std::path::PathBuf;

use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
use crate::view::Kind;
use crate::{BUNDLED_ADWAITA_DARK, BUNDLED_ADWAITA_HC, BUNDLED_ADWAITA_LIGHT};

/// Which bundled Adwaita sheet the gallery compiles, and under which
/// `@media` environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// `--theme light` (the default).
    Light,
    /// `--theme dark`.
    Dark,
    /// `--theme hc`.
    HighContrast,
}

impl Theme {
    /// The CLI spelling, exactly; case-sensitive so a typo is an error rather
    /// than a silent fallback to light.
    #[must_use]
    pub fn parse(text: &str) -> Option<Theme> {
        match text {
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            "hc" => Some(Theme::HighContrast),
            _ => None,
        }
    }

    /// The spelling `--theme` accepts and the gates name their captures by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::HighContrast => "hc",
        }
    }

    /// The vendored sheet source. Never the developer's own `gtk.css`: the
    /// gate's colours are pinned to these three files.
    #[must_use]
    pub fn sheet(self) -> &'static str {
        match self {
            Theme::Light => BUNDLED_ADWAITA_LIGHT,
            Theme::Dark => BUNDLED_ADWAITA_DARK,
            Theme::HighContrast => BUNDLED_ADWAITA_HC,
        }
    }

    /// The `@media` environment the sheet is compiled under, so
    /// `prefers-color-scheme`/`prefers-contrast` blocks inside it apply.
    #[must_use]
    pub fn media_env(self) -> MediaEnv {
        match self {
            Theme::Light => MediaEnv {
                color_scheme: ColorScheme::Light,
                contrast: Contrast::NoPreference,
            },
            Theme::Dark => MediaEnv {
                color_scheme: ColorScheme::Dark,
                contrast: Contrast::NoPreference,
            },
            Theme::HighContrast => MediaEnv {
                color_scheme: ColorScheme::Light,
                contrast: Contrast::More,
            },
        }
    }
}

/// Why an argument list was rejected. Rejection is always total: the gallery
/// never guesses a value it could not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionsError {
    /// A flag that takes a value came last.
    MissingValue(&'static str),
    /// A value that could not be read as what the flag needs.
    BadValue(&'static str, String),
    /// An argument the gallery does not define.
    Unknown(String),
}

impl std::fmt::Display for OptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptionsError::MissingValue(flag) => write!(f, "{flag} needs a value"),
            OptionsError::BadValue(flag, value) => write!(f, "{flag}: cannot read {value:?}"),
            OptionsError::Unknown(arg) => write!(f, "unknown argument {arg:?}"),
        }
    }
}

impl std::error::Error for OptionsError {}

/// The `gallery` command line, as documented in the M3 contract §7.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Which bundled sheet to compile.
    pub theme: Theme,
    /// A sheet on disk, used *whole*, instead of the bundled one.
    pub theme_file: Option<PathBuf>,
    /// Render exactly one widget, alone, at the origin.
    pub widget: Option<Kind>,
    /// Print every widget name and exit.
    pub list: bool,
    /// Print `<widget> <label> <x> <y>` for every probe point and exit.
    pub probe_points: bool,
    /// Print `<widget> <x> <y> <width> <height>` per entry and exit.
    pub print_allocation: bool,
    /// Surface size.
    pub size: (u32, u32),
    /// Scroll the page before the first frame, in px.
    pub scroll: i32,
    /// Output scale, for HiDPI probes.
    pub scale: i32,
}

impl Options {
    /// `--size`'s default, and the surface the gates capture against.
    pub const DEFAULT_SIZE: (u32, u32) = (1280, 800);

    /// Parse an argument list (without `argv[0]`).
    ///
    /// # Errors
    ///
    /// [`OptionsError`] for a missing value, an unreadable value or an
    /// unknown flag. Nothing here panics on any input: the argument list is
    /// the one piece of untrusted data this binary reads.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, OptionsError> {
        let mut opts = Options {
            theme: Theme::Light,
            theme_file: None,
            widget: None,
            list: false,
            probe_points: false,
            print_allocation: false,
            size: Options::DEFAULT_SIZE,
            scroll: 0,
            scale: 1,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => opts.list = true,
                "--probe-points" => opts.probe_points = true,
                "--print-allocation" => opts.print_allocation = true,
                "--theme" => {
                    let value = args.next().ok_or(OptionsError::MissingValue("--theme"))?;
                    opts.theme = Theme::parse(&value)
                        .ok_or(OptionsError::BadValue("--theme", value.clone()))?;
                }
                "--theme-file" => {
                    let value = args
                        .next()
                        .ok_or(OptionsError::MissingValue("--theme-file"))?;
                    if value.is_empty() {
                        return Err(OptionsError::BadValue("--theme-file", value));
                    }
                    opts.theme_file = Some(PathBuf::from(value));
                }
                "--widget" => {
                    let value = args.next().ok_or(OptionsError::MissingValue("--widget"))?;
                    let kind = kind_from_name(&value)
                        .ok_or_else(|| OptionsError::BadValue("--widget", value.clone()))?;
                    // A sub-kind has no standalone rendering: it exists only
                    // inside its parent's entry, so asking for one alone is an
                    // error rather than an empty surface.
                    if !matches!(sample_shape(kind), SampleShape::Own) {
                        return Err(OptionsError::BadValue("--widget", value));
                    }
                    opts.widget = Some(kind);
                }
                "--size" => {
                    let value = args.next().ok_or(OptionsError::MissingValue("--size"))?;
                    opts.size = parse_size(&value)
                        .ok_or(OptionsError::BadValue("--size", value.clone()))?;
                }
                "--scroll" => {
                    let value = args.next().ok_or(OptionsError::MissingValue("--scroll"))?;
                    opts.scroll = value
                        .parse::<i32>()
                        .map_err(|_| OptionsError::BadValue("--scroll", value.clone()))?;
                }
                "--scale" => {
                    let value = args.next().ok_or(OptionsError::MissingValue("--scale"))?;
                    let scale = value
                        .parse::<i32>()
                        .map_err(|_| OptionsError::BadValue("--scale", value.clone()))?;
                    if !(1..=4).contains(&scale) {
                        return Err(OptionsError::BadValue("--scale", value));
                    }
                    opts.scale = scale;
                }
                other => return Err(OptionsError::Unknown(other.to_string())),
            }
        }
        Ok(opts)
    }
}

/// `<width>x<height>`, both positive and both inside a 16k surface.
///
/// `str::parse` is the only integer path: no slicing, no `unwrap`, so a
/// multi-byte or absurd value is an `Err`, never a panic.
fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once('x')?;
    let width: u32 = w.parse().ok()?;
    let height: u32 = h.parse().ok()?;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }
    Some((width, height))
}

/// The CLI name of `kind`: the `Kind`'s own snake_case spelling.
///
/// Note `Kind::Box` is `"box"` here while its builder is `box_` — the builder
/// carries the trailing underscore only because `box` is a Rust keyword.
#[must_use]
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        // P5 · display
        Kind::Label => "label",
        Kind::Spinner => "spinner",
        Kind::Statusbar => "statusbar",
        Kind::LevelBar => "level_bar",
        Kind::ProgressBar => "progress_bar",
        Kind::InfoBar => "info_bar",
        Kind::Scrollbar => "scrollbar",
        Kind::Image => "image",
        Kind::Picture => "picture",
        Kind::Separator => "separator",
        Kind::TextView => "text_view",
        Kind::Scale => "scale",
        Kind::DrawingArea => "drawing_area",
        Kind::WindowControls => "window_controls",
        Kind::Calendar => "calendar",
        Kind::Popover => "popover",
        // P5 · buttons
        Kind::Button => "button",
        Kind::ToggleButton => "toggle_button",
        Kind::LinkButton => "link_button",
        Kind::CheckButton => "check_button",
        Kind::MenuButton => "menu_button",
        Kind::Switch => "switch",
        Kind::DropDown => "drop_down",
        Kind::ColorDialogButton => "color_dialog_button",
        Kind::ColorDialog => "color_dialog",
        Kind::FontDialogButton => "font_dialog_button",
        Kind::FontDialog => "font_dialog",
        // P5 · entries
        Kind::Entry => "entry",
        Kind::SearchEntry => "search_entry",
        Kind::PasswordEntry => "password_entry",
        Kind::SpinButton => "spin_button",
        Kind::EditableLabel => "editable_label",
        // P6 · containers
        Kind::Box => "box",
        Kind::Grid => "grid",
        Kind::CenterBox => "center_box",
        Kind::ScrolledWindow => "scrolled_window",
        Kind::Paned => "paned",
        Kind::Frame => "frame",
        Kind::Expander => "expander",
        Kind::SearchBar => "search_bar",
        Kind::ActionBar => "action_bar",
        Kind::HeaderBar => "header_bar",
        Kind::Notebook => "notebook",
        Kind::NotebookTab => "notebook_tab",
        Kind::Overlay => "overlay",
        Kind::Stack => "stack",
        Kind::StackPage => "stack_page",
        Kind::StackSwitcher => "stack_switcher",
        Kind::StackSidebar => "stack_sidebar",
        // P6 · lists
        Kind::ListBox => "list_box",
        Kind::ListBoxRow => "list_box_row",
        Kind::FlowBox => "flow_box",
        Kind::FlowBoxChild => "flow_box_child",
        Kind::ListView => "list_view",
        Kind::GridView => "grid_view",
        Kind::ColumnView => "column_view",
        Kind::ColumnViewColumn => "column_view_column",
        // P6 · menus
        Kind::PopoverMenu => "popover_menu",
        Kind::PopoverMenuBar => "popover_menu_bar",
        Kind::PopoverMenuItem => "popover_menu_item",
        // P6 · windows
        Kind::Window => "window",
        Kind::ShortcutsWindow => "shortcuts_window",
        Kind::AboutDialog => "about_dialog",
        Kind::AlertDialog => "alert_dialog",
    }
}

/// The inverse of [`kind_name`], by linear scan over `Kind::all()` — 64 string
/// comparisons at startup, which is cheaper than a map to maintain.
#[must_use]
pub fn kind_from_name(name: &str) -> Option<Kind> {
    Kind::all().iter().copied().find(|&k| kind_name(k) == name)
}

/// Whether a kind is its own gallery entry or only ever a sub-node of another.
///
/// Task 2 gives this its real body; Task 1 needs only the `Own` answer for
/// `--widget`'s validation, and `sample()` in Task 2 is the single source of
/// truth both share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleShape {
    /// Rendered as its own framed entry on the page.
    Own,
    /// Rendered only inside the named parent kind's entry.
    Within(Kind),
}

/// Every widget name, one per line, in `Kind::all()` order.
pub fn print_list() {
    for &kind in Kind::all() {
        println!("{}", kind_name(kind));
    }
}

/// The six sub-kinds GTK renders as their own node inside a parent widget
/// (§4.2); everything else is its own entry.
#[must_use]
pub fn sample_shape(kind: Kind) -> SampleShape {
    match kind {
        Kind::NotebookTab => SampleShape::Within(Kind::Notebook),
        Kind::StackPage => SampleShape::Within(Kind::Stack),
        Kind::ListBoxRow => SampleShape::Within(Kind::ListBox),
        Kind::FlowBoxChild => SampleShape::Within(Kind::FlowBox),
        Kind::ColumnViewColumn => SampleShape::Within(Kind::ColumnView),
        Kind::PopoverMenuItem => SampleShape::Within(Kind::PopoverMenu),
        _ => SampleShape::Own,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn defaults_match_the_documented_defaults() {
        let opts = Options::parse(args(&[])).expect("no arguments parses");
        assert_eq!(opts.theme, Theme::Light);
        assert_eq!(opts.size, (1280, 800));
        assert_eq!(opts.scale, 1);
        assert_eq!(opts.scroll, 0);
        assert!(opts.widget.is_none());
        assert!(opts.theme_file.is_none());
        assert!(!opts.list && !opts.probe_points && !opts.print_allocation);
    }

    #[test]
    fn every_documented_option_parses() {
        let opts = Options::parse(args(&[
            "--theme",
            "hc",
            "--widget",
            "check_button",
            "--size",
            "640x480",
            "--scroll",
            "120",
            "--scale",
            "2",
            "--probe-points",
        ]))
        .expect("the documented options parse");
        assert_eq!(opts.theme, Theme::HighContrast);
        assert_eq!(opts.widget, Some(Kind::CheckButton));
        assert_eq!(opts.size, (640, 480));
        assert_eq!(opts.scroll, 120);
        assert_eq!(opts.scale, 2);
        assert!(opts.probe_points);
    }

    #[test]
    fn theme_file_wins_over_theme_but_both_are_kept() {
        let opts = Options::parse(args(&["--theme", "dark", "--theme-file", "/tmp/x.css"]))
            .expect("both parse");
        assert_eq!(opts.theme, Theme::Dark);
        assert_eq!(opts.theme_file, Some(PathBuf::from("/tmp/x.css")));
    }

    /// Never-panic gate for the one piece of untrusted input this part parses.
    ///
    /// Mutation check: change `--size`'s parser to `text[..i].parse().unwrap()`
    /// and this test panics instead of failing cleanly; restore.
    #[test]
    fn hostile_argv_is_rejected_without_panicking() {
        let hostile: &[&[&str]] = &[
            &["--size"],
            &["--size", ""],
            &["--size", "x"],
            &["--size", "0x0"],
            &["--size", "99999999999999999999x1"],
            &["--size", "-1x-1"],
            &["--size", "12x34x56"],
            &["--size", "١٢x٣٤"],
            &["--scale"],
            &["--scale", "0"],
            &["--scale", "-3"],
            &["--scale", "1.5"],
            &["--scroll", "nope"],
            &["--theme"],
            &["--theme", "puce"],
            &["--theme-file"],
            &["--widget"],
            &["--widget", "✂"],
            &["--widget", "list_box_row"],
            &["--frobnicate"],
            &["-"],
            &["--"],
            &["\u{0}"],
        ];
        for case in hostile {
            let parsed = Options::parse(args(case));
            assert!(parsed.is_err(), "{case:?} should not parse, got {parsed:?}");
            // Displaying the error must not panic either.
            let _ = parsed.unwrap_err().to_string();
        }
    }

    #[test]
    fn every_kind_has_a_unique_name_that_round_trips() {
        let mut seen = std::collections::BTreeSet::new();
        for &kind in Kind::all() {
            let name = kind_name(kind);
            assert!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{name:?} is not a snake_case name"
            );
            assert!(seen.insert(name), "two kinds share the name {name:?}");
            assert_eq!(
                kind_from_name(name),
                Some(kind),
                "{name} does not round-trip"
            );
        }
        assert_eq!(
            seen.len(),
            Kind::all().len(),
            "kind_name is not total over Kind::all()"
        );
        assert!(
            Kind::all().len() >= 59,
            "the contract's R1 puts the in-scope count at ~59 kinds; got {}",
            Kind::all().len()
        );
    }

    #[test]
    fn themes_map_to_three_distinct_sheets_and_environments() {
        assert_eq!(Theme::parse("light"), Some(Theme::Light));
        assert_eq!(Theme::parse("dark"), Some(Theme::Dark));
        assert_eq!(Theme::parse("hc"), Some(Theme::HighContrast));
        assert_eq!(Theme::parse("HC"), None, "theme names are case-sensitive");
        assert_ne!(Theme::Light.sheet(), Theme::Dark.sheet());
        assert_ne!(Theme::Dark.sheet(), Theme::HighContrast.sheet());
        assert_eq!(Theme::Dark.media_env().color_scheme, ColorScheme::Dark);
        assert_eq!(Theme::HighContrast.media_env().contrast, Contrast::More);
    }
}
