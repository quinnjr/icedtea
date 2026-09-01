//! The `gallery` binary, as library code.
//!
//! Lives in the library rather than in `src/bin/gallery.rs` because [`Kind`]
//! is `#[non_exhaustive]`: an exhaustive `match` over it compiles only inside
//! the crate that defines it, and "a `Kind` with no gallery entry does not
//! compile" is the completeness guarantee the M3 gate rests on.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;

use skia_rs_safe::core::Color;
use skia_rs_safe::paint::Paint;

use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
use crate::css::value::{IconRef, Rgba};
use crate::layout::Rect;
use crate::view::builders::{
    self as w, CheckButtonExt, ColorDialogButtonExt, DropDownExt, ImageExt, InfoBarExt, LabelExt,
    LevelBarExt, MenuButtonExt, PopoverExt, ProgressBarExt, ScaleExt, ScrollbarExt, SpinnerExt,
    StatusbarExt, TextViewExt, ToggleButtonExt,
};
use crate::view::cmd::Cmd;
use crate::view::{Kind, View};
use crate::widgets::types::{MessageType, Orientation, Side};
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

/// Everything the gallery's widgets can say. One variant per `EventKind` the
/// samples bind, carrying the `Kind` that spoke so the interaction gate can
/// name it.
#[derive(Clone, Debug, PartialEq)]
pub enum GalleryMsg {
    /// A button-ish widget was clicked.
    Clicked(Kind),
    /// A toggle/check/switch changed state.
    Toggled(Kind, bool),
    /// An editable widget's text changed.
    Changed(Kind, String),
    /// A list-ish widget's selection changed.
    Selected(Kind, usize),
    /// A range widget's value changed.
    ValueChanged(Kind, f64),
    /// `EventKind::Activate` (Space/Enter, or a completed click).
    Activated(Kind),
    /// A `SearchEntry` fired after its delay.
    Search(String),
    /// A `Notebook`/`Stack` page changed.
    PageChanged(usize),
    /// An `Expander` opened or closed.
    Expanded(bool),
    /// A dialog-ish widget asked to close.
    Closed(Kind),
}

impl GalleryMsg {
    /// The one line this message prints on stdout.
    ///
    /// Flat ASCII-ish, whitespace-separated, newlines in payloads folded to
    /// spaces: the interaction gate matches on substrings of these lines and a
    /// payload must not be able to forge a second line.
    #[must_use]
    pub fn log_line(&self) -> String {
        fn flat(text: &str) -> String {
            text.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect()
        }
        match self {
            GalleryMsg::Clicked(k) => format!("clicked {}", kind_name(*k)),
            GalleryMsg::Toggled(k, on) => format!("toggled {} {on}", kind_name(*k)),
            GalleryMsg::Changed(k, text) => format!("changed {} {}", kind_name(*k), flat(text)),
            GalleryMsg::Selected(k, i) => format!("selected {} {i}", kind_name(*k)),
            GalleryMsg::ValueChanged(k, v) => format!("value {} {v}", kind_name(*k)),
            GalleryMsg::Activated(k) => format!("activated {}", kind_name(*k)),
            GalleryMsg::Search(text) => format!("search {}", flat(text)),
            GalleryMsg::PageChanged(i) => format!("page {i}"),
            GalleryMsg::Expanded(on) => format!("expanded {on}"),
            GalleryMsg::Closed(k) => format!("closed {}", kind_name(*k)),
        }
    }
}

/// The gallery's whole model: per-kind widget state plus the message log.
///
/// Keyed by [`kind_name`] rather than by `Kind` so a `BTreeMap` debug dump
/// reads the way the gate's assertions do.
#[derive(Clone, Debug, PartialEq)]
pub struct GalleryModel {
    /// Which sheet is compiled; the samples do not read it, the runner does.
    pub theme: Theme,
    /// `--widget`'s kind, when the page holds exactly one widget.
    pub only: Option<Kind>,
    /// Checked/active state per kind.
    pub toggles: BTreeMap<&'static str, bool>,
    /// Editable text per kind.
    pub texts: BTreeMap<&'static str, String>,
    /// Range value per kind.
    pub values: BTreeMap<&'static str, f64>,
    /// Selected index per kind.
    pub selected: BTreeMap<&'static str, usize>,
    /// The visible `Notebook`/`Stack` page.
    pub page: usize,
    /// The `Expander`'s state.
    pub expanded: bool,
    /// `--scroll`, in px. Lives on the model because `App::new` takes a
    /// `fn(&M) -> View<Msg>` pointer (§4.7), which cannot capture it.
    pub scroll: i32,
    /// Every folded message's [`GalleryMsg::log_line`], in order.
    pub log: Vec<String>,
}

impl GalleryModel {
    /// A model with every widget in its documented initial state.
    #[must_use]
    pub fn new(theme: Theme, only: Option<Kind>) -> Self {
        let mut model = GalleryModel {
            theme,
            only,
            toggles: BTreeMap::new(),
            texts: BTreeMap::new(),
            values: BTreeMap::new(),
            selected: BTreeMap::new(),
            page: 0,
            expanded: false,
            scroll: 0,
            log: Vec::new(),
        };
        model
            .texts
            .insert(kind_name(Kind::Entry), "Entry".to_string());
        model
            .texts
            .insert(kind_name(Kind::TextView), "Text view".to_string());
        model
            .texts
            .insert(kind_name(Kind::PasswordEntry), "hunter2".to_string());
        model
            .texts
            .insert(kind_name(Kind::EditableLabel), "Editable".to_string());
        model.values.insert(kind_name(Kind::Scale), 40.0);
        model.values.insert(kind_name(Kind::SpinButton), 3.0);
        model
    }

    /// This kind's checked/active state; `false` until something toggles it.
    #[must_use]
    pub fn toggle(&self, kind: Kind) -> bool {
        self.toggles.get(kind_name(kind)).copied().unwrap_or(false)
    }

    /// This kind's text; `""` until something sets it.
    #[must_use]
    pub fn text(&self, kind: Kind) -> &str {
        self.texts.get(kind_name(kind)).map_or("", String::as_str)
    }

    /// This kind's range value; `0.0` until something sets it.
    #[must_use]
    pub fn value(&self, kind: Kind) -> f64 {
        self.values.get(kind_name(kind)).copied().unwrap_or(0.0)
    }

    /// This kind's selected index; `0` until something selects.
    #[must_use]
    pub fn selection(&self, kind: Kind) -> usize {
        self.selected.get(kind_name(kind)).copied().unwrap_or(0)
    }
}

/// Fold one message into the model and print its log line.
///
/// The print is the whole reason the gate can make *model* assertions across
/// a process boundary; stdout is block-buffered when piped, so every line is
/// flushed immediately.
pub fn update(model: &mut GalleryModel, msg: GalleryMsg) -> Cmd<GalleryMsg> {
    let line = msg.log_line();
    match msg {
        GalleryMsg::Toggled(kind, on) => {
            model.toggles.insert(kind_name(kind), on);
        }
        GalleryMsg::Changed(kind, text) => {
            model.texts.insert(kind_name(kind), text);
        }
        GalleryMsg::ValueChanged(kind, value) => {
            model.values.insert(kind_name(kind), value);
        }
        GalleryMsg::Selected(kind, index) => {
            model.selected.insert(kind_name(kind), index);
        }
        GalleryMsg::PageChanged(index) => model.page = index,
        GalleryMsg::Expanded(on) => model.expanded = on,
        GalleryMsg::Clicked(_)
        | GalleryMsg::Activated(_)
        | GalleryMsg::Search(_)
        | GalleryMsg::Closed(_) => {}
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "msg {line}");
    let _ = out.flush();
    model.log.push(line);
    Cmd::None
}

/// A kind's place in the gallery.
pub enum Sample {
    /// Its own framed entry, rooted at a view of that kind.
    Own(View<GalleryMsg>),
    /// Rendered only inside the named parent kind's entry.
    Within(Kind),
}

/// A 4x4 opaque `#e01b24` PNG, so the `Picture` sample has something real to
/// decode without vendoring a binary fixture the gallery cannot find at
/// runtime.
pub const SAMPLE_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x06, 0x00, 0x00, 0x00, 0xa9, 0xf1, 0x9e,
    0x7e, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x78, 0x20, 0xad, 0xf2,
    0x1f, 0x19, 0x33, 0x90, 0x2e, 0x00, 0x00, 0x7d, 0x10, 0x21, 0xe1, 0xa6, 0x7b, 0xaf, 0xe4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Where [`SAMPLE_PNG`] lives on disk, written on first ask.
///
/// A write failure is not fatal: `Picture` then shows nothing and only that
/// one probe fails, which is a better failure than a dead gallery.
#[must_use]
pub fn sample_png_path() -> PathBuf {
    let path = std::env::temp_dir().join("icedtea-gallery-sample.png");
    let fresh = std::fs::read(&path)
        .ok()
        .is_none_or(|got| got != SAMPLE_PNG);
    if fresh && let Err(err) = std::fs::write(&path, SAMPLE_PNG) {
        tracing::warn!(path = %path.display(), %err, "cannot write the gallery's sample png");
    }
    path
}

/// The exhaustive sample table: one entry per `Kind`.
///
/// Exhaustive on purpose — a new `Kind` fails to compile here, which is how
/// the gallery stays a completeness measure rather than a demo.
#[must_use]
pub fn sample(kind: Kind, model: &GalleryModel) -> Sample {
    match kind {
        // ---- P5 · display ------------------------------------------------
        Kind::Label => Sample::Own(w::label("Label").wrap(true).xalign(0.0)),
        Kind::Spinner => Sample::Own(w::spinner().spinning(true)),
        Kind::Statusbar => Sample::Own(StatusbarExt::text(w::statusbar(), "Ready")),
        Kind::LevelBar => Sample::Own(
            w::level_bar(0.6)
                .min_value(0.0)
                .max_value(1.0)
                .width_request(160),
        ),
        Kind::ProgressBar => Sample::Own(w::progress_bar(0.4).show_text(true).width_request(160)),
        Kind::InfoBar => Sample::Own(
            w::info_bar()
                .message_type(MessageType::Info)
                .revealed(true)
                .show_close_button(true)
                .on_close(GalleryMsg::Closed(Kind::InfoBar))
                .child(w::label("An info bar")),
        ),
        Kind::Scrollbar => Sample::Own(
            ScrollbarExt::value(w::scrollbar(Orientation::Horizontal), 0.3)
                .lower(0.0)
                .upper(1.0)
                .page_size(0.25)
                .width_request(160)
                .on_value_changed(|v| GalleryMsg::ValueChanged(Kind::Scrollbar, v)),
        ),
        Kind::Image => Sample::Own(
            w::image(IconRef::Theme {
                name: Rc::from("folder"),
            })
            .pixel_size(32),
        ),
        Kind::Picture => Sample::Own(
            w::picture(&sample_png_path())
                .width_request(48)
                .height_request(48),
        ),
        Kind::Separator => Sample::Own(w::separator(Orientation::Horizontal).width_request(160)),
        Kind::TextView => Sample::Own(
            w::text_view(model.text(Kind::TextView))
                .editable(true)
                .width_request(200)
                .height_request(64)
                .on_change(|t: &str| GalleryMsg::Changed(Kind::TextView, t.to_string())),
        ),
        Kind::Scale => Sample::Own(
            ScaleExt::value(w::scale(0.0, 100.0), model.value(Kind::Scale))
                .orientation(Orientation::Horizontal)
                .width_request(200)
                .on_value_changed(|v| GalleryMsg::ValueChanged(Kind::Scale, v)),
        ),
        Kind::DrawingArea => Sample::Own(
            w::drawing_area(
                |canvas: &mut skia_rs_safe::canvas::Canvas<'_>, rect: Rect| {
                    let mut paint = Paint::new();
                    paint.set_color32(Color(0xFF33_D17A));
                    canvas.draw_rect(&rect.to_skia(), &paint);
                },
            )
            .width_request(64)
            .height_request(48),
        ),
        Kind::WindowControls => Sample::Own(w::window_controls(Side::End)),
        Kind::Calendar => Sample::Own(
            w::calendar(2026, 8, 27)
                .on_date_selected(|iso: &str| GalleryMsg::Changed(Kind::Calendar, iso.to_string())),
        ),
        // A non-autohide popover renders into the parent window's own tree
        // (§5.1's ruling), which is what gives it pixels at rest.
        Kind::Popover => Sample::Own(
            w::popover(w::label("Popover"))
                .autohide(false)
                .has_arrow(true),
        ),

        // ---- P5 · buttons ------------------------------------------------
        Kind::Button => {
            Sample::Own(w::button("Button").on_click(GalleryMsg::Clicked(Kind::Button)))
        }
        Kind::ToggleButton => Sample::Own(
            ToggleButtonExt::active(w::toggle_button("Toggle"), model.toggle(Kind::ToggleButton))
                .on_toggle(|on| GalleryMsg::Toggled(Kind::ToggleButton, on)),
        ),
        Kind::LinkButton => Sample::Own(
            w::link_button("https://example.invalid/", "Link")
                .on_activate_link(|_uri: &str| GalleryMsg::Clicked(Kind::LinkButton)),
        ),
        Kind::CheckButton => Sample::Own(
            CheckButtonExt::active(w::check_button("Check"), model.toggle(Kind::CheckButton))
                .on_toggle(|on| GalleryMsg::Toggled(Kind::CheckButton, on)),
        ),
        Kind::MenuButton => Sample::Own(
            w::menu_button("Menu")
                .always_show_arrow(true)
                .child(w::label("Menu content")),
        ),
        Kind::Switch => Sample::Own(
            w::switch(model.toggle(Kind::Switch))
                .on_toggle(|on| GalleryMsg::Toggled(Kind::Switch, on)),
        ),
        Kind::DropDown => Sample::Own(
            w::drop_down(&["One", "Two", "Three"])
                .selected(model.selection(Kind::DropDown))
                .on_selected(|i| GalleryMsg::Selected(Kind::DropDown, i)),
        ),
        Kind::ColorDialogButton => Sample::Own(ColorDialogButtonExt::on_change(
            w::color_dialog_button(Rgba {
                r: 0.21,
                g: 0.52,
                b: 0.89,
                a: 1.0,
            }),
            |v: f64| GalleryMsg::ValueChanged(Kind::ColorDialogButton, v),
        )),
        Kind::ColorDialog => Sample::Own(w::color_dialog(Rgba {
            r: 0.21,
            g: 0.52,
            b: 0.89,
            a: 1.0,
        })),
        Kind::FontDialogButton => Sample::Own(
            w::font_dialog_button("Cantarell 11")
                .on_change(|s: &str| GalleryMsg::Changed(Kind::FontDialogButton, s.to_string())),
        ),
        Kind::FontDialog => Sample::Own(w::font_dialog("Cantarell 11")),

        // ---- P5 · entries ------------------------------------------------
        //
        // Reconciliation: several P5/P6 traits share a method name over an
        // unconstrained `impl<Msg> Trait for View<Msg>` (EntryExt::
        // placeholder / SearchEntryExt::placeholder / PasswordEntryExt::
        // placeholder; SpinButtonExt::{digits,wrap,page,orientation} against
        // LabelExt/ScaleExt/NotebookExt), so bringing every one of those
        // traits into this file's scope at once (as the task text's plain
        // `.method()` chains assume) makes plain dot calls ambiguous --
        // `cargo build` reports E0034 at each collision, including on
        // pre-existing P5 arms the collision never touched before. Rather
        // than import the traits and disambiguate every call, no new trait
        // is `use`-imported here: each one is named by its full path at the
        // call site (`crate::widgets::entry::EntryExt::placeholder(..)`, or
        // `w::BoxExt::spacing(..)` for a `view::builders` trait, since `w`
        // is that module's alias), which resolves unambiguously without
        // touching any other arm's scope.
        Kind::Entry => Sample::Own(
            crate::widgets::entry::EntryExt::width_chars(
                crate::widgets::entry::EntryExt::placeholder(
                    w::entry(model.text(Kind::Entry)),
                    "Type here",
                ),
                16,
            )
            .on_change(|t: &str| GalleryMsg::Changed(Kind::Entry, t.to_string()))
            .on_activate(GalleryMsg::Activated(Kind::Entry)),
        ),
        Kind::SearchEntry => Sample::Own(
            crate::widgets::search_entry::SearchEntryExt::placeholder(
                w::search_entry(model.text(Kind::SearchEntry)),
                "Search",
            )
            .on_search(|t: &str| GalleryMsg::Search(t.to_string()))
            .on_change(|t: &str| GalleryMsg::Changed(Kind::SearchEntry, t.to_string())),
        ),
        Kind::PasswordEntry => Sample::Own(
            crate::widgets::password_entry::PasswordEntryExt::show_peek_icon(
                w::password_entry(model.text(Kind::PasswordEntry)),
                true,
            )
            .on_change(|t: &str| GalleryMsg::Changed(Kind::PasswordEntry, t.to_string())),
        ),
        Kind::SpinButton => Sample::Own(
            crate::widgets::spin_button::SpinButtonExt::digits(
                crate::widgets::spin_button::SpinButtonExt::step(
                    w::spin_button(model.value(Kind::SpinButton), 0.0, 10.0),
                    1.0,
                ),
                0,
            )
            .on_value_changed(|v| GalleryMsg::ValueChanged(Kind::SpinButton, v)),
        ),
        Kind::EditableLabel => Sample::Own(
            w::editable_label(model.text(Kind::EditableLabel))
                .on_change(|t: &str| GalleryMsg::Changed(Kind::EditableLabel, t.to_string())),
        ),

        // ---- P6 · containers ---------------------------------------------
        Kind::Box => Sample::Own(w::BoxExt::spacing(
            w::box_(
                Orientation::Horizontal,
                [w::label("One"), w::label("Two"), w::label("Three")],
            ),
            6,
        )),
        Kind::Grid => Sample::Own(w::GridExt::column_spacing(
            w::GridExt::row_spacing(
                w::grid([
                    w::GridExt::at(w::label("0,0"), 0, 0),
                    w::GridExt::at(w::label("1,0"), 1, 0),
                    w::GridExt::span(w::GridExt::at(w::label("wide"), 0, 1), 2, 1),
                ]),
                6,
            ),
            6,
        )),
        Kind::CenterBox => Sample::Own(
            w::center_box(w::label("start"), w::label("centre"), w::label("end"))
                .prop(crate::view::PropName::Orientation, Orientation::Horizontal)
                .width_request(240),
        ),
        Kind::ScrolledWindow => Sample::Own(w::ScrolledWindowExt::min_content_height(
            w::ScrolledWindowExt::min_content_width(
                w::scrolled_window(w::box_(
                    Orientation::Vertical,
                    (0..12).map(|i| w::label(&format!("Row {i}")).key(i as usize)),
                )),
                180,
            ),
            80,
        )),
        Kind::Paned => Sample::Own(
            w::PanedExt::position(
                w::paned(Orientation::Horizontal, w::label("left"), w::label("right")),
                90,
            )
            .width_request(200)
            .height_request(60),
        ),
        Kind::Frame => Sample::Own(crate::widgets::button::ButtonExt::label(
            w::frame(w::label("Framed")),
            "Frame",
        )),
        Kind::Expander => Sample::Own(
            crate::widgets::expander::ExpanderExt::expanded(
                w::expander("Expander", w::label("Revealed")),
                model.expanded,
            )
            .on_expanded(GalleryMsg::Expanded),
        ),
        Kind::SearchBar => Sample::Own(
            crate::widgets::search_bar::SearchBarExt::search_mode(
                w::search_bar(w::search_entry("")),
                true,
            )
            .width_request(240),
        ),
        Kind::ActionBar => Sample::Own(
            w::PackExt::pack_end(
                w::PackExt::pack_start(
                    crate::widgets::action_bar::ActionBarExt::revealed(w::action_bar(), true),
                    w::button("Start").on_click(GalleryMsg::Clicked(Kind::ActionBar)),
                ),
                w::button("End").on_click(GalleryMsg::Clicked(Kind::ActionBar)),
            )
            .width_request(240),
        ),
        Kind::HeaderBar => Sample::Own(
            w::PackExt::pack_start(
                w::HeaderBarExt::subtitle(w::HeaderBarExt::title(w::header_bar(), "Header"), "bar"),
                w::button("Back").on_click(GalleryMsg::Clicked(Kind::HeaderBar)),
            )
            .width_request(280),
        ),
        Kind::Notebook => Sample::Own(
            w::NotebookExt::page(
                w::notebook([
                    w::notebook_tab("One", w::label("Page one")).key(0usize),
                    w::notebook_tab("Two", w::label("Page two")).key(1usize),
                ]),
                model.page,
            )
            .on_page_changed(GalleryMsg::PageChanged)
            .width_request(240)
            .height_request(100),
        ),
        Kind::NotebookTab => Sample::Within(Kind::Notebook),
        Kind::Overlay => Sample::Own(
            w::OverlayExt::overlay(
                w::overlay(w::label("Under")),
                w::label("Over").halign(crate::layout::Align::End),
            )
            .width_request(160)
            .height_request(48),
        ),
        Kind::Stack => Sample::Own(
            w::StackExt::transition_duration(
                w::StackExt::transition_type(
                    w::StackExt::visible_child(
                        w::stack([
                            w::stack_page("one", "One", w::label("Page one")).key("one"),
                            w::stack_page("two", "Two", w::label("Page two")).key("two"),
                        ]),
                        if model.page == 0 { "one" } else { "two" },
                    ),
                    crate::widgets::types::StackTransition::SlideLeftRight,
                ),
                120,
            )
            .on_change(|name: &str| GalleryMsg::PageChanged(usize::from(name == "two")))
            .width_request(200)
            .height_request(60),
        ),
        Kind::StackPage => Sample::Within(Kind::Stack),
        Kind::StackSwitcher => Sample::Own(
            w::StackPagesExt::selected(w::stack_switcher(stack_pages()), model.page)
                .on_selected(GalleryMsg::PageChanged),
        ),
        Kind::StackSidebar => Sample::Own(
            w::StackPagesExt::selected(w::stack_sidebar(stack_pages()), model.page)
                .on_selected(GalleryMsg::PageChanged)
                .width_request(120)
                .height_request(80),
        ),

        // Task 4 replaces this arm with the remaining 15.
        _ => Sample::Within(Kind::Box),
    }
}

/// The two pages `StackSwitcher` and `StackSidebar` present.
fn stack_pages() -> Rc<[crate::widgets::types::StackPageInfo]> {
    Rc::from(
        [
            crate::widgets::types::StackPageInfo {
                name: Rc::from("one"),
                title: Rc::from("One"),
                icon: None,
                needs_attention: false,
            },
            crate::widgets::types::StackPageInfo {
                name: Rc::from("two"),
                title: Rc::from("Two"),
                icon: None,
                needs_attention: false,
            },
        ]
        .as_slice(),
    )
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

    #[test]
    fn every_display_and_button_kind_has_its_own_sample() {
        let model = GalleryModel::new(Theme::Light, None);
        let covered = [
            Kind::Label,
            Kind::Spinner,
            Kind::Statusbar,
            Kind::LevelBar,
            Kind::ProgressBar,
            Kind::InfoBar,
            Kind::Scrollbar,
            Kind::Image,
            Kind::Picture,
            Kind::Separator,
            Kind::TextView,
            Kind::Scale,
            Kind::DrawingArea,
            Kind::WindowControls,
            Kind::Calendar,
            Kind::Popover,
            Kind::Button,
            Kind::ToggleButton,
            Kind::LinkButton,
            Kind::CheckButton,
            Kind::MenuButton,
            Kind::Switch,
            Kind::DropDown,
            Kind::ColorDialogButton,
            Kind::ColorDialog,
            Kind::FontDialogButton,
            Kind::FontDialog,
        ];
        for kind in covered {
            match sample(kind, &model) {
                Sample::Own(view) => assert_eq!(
                    view.kind,
                    kind,
                    "{}'s sample must be rooted at its own kind",
                    kind_name(kind)
                ),
                Sample::Within(parent) => {
                    panic!("{} is not a sub-kind of {parent:?}", kind_name(kind))
                }
            }
        }
    }

    /// The model is what makes the interaction gate's assertions readable:
    /// every interactive sample reads its state back out of it.
    ///
    /// Mutation check: make `update`'s `Toggled` arm ignore its `bool` and
    /// always store `true`; this test fails on the second assertion. Restore.
    #[test]
    fn update_folds_state_and_records_one_log_line_per_message() {
        let mut model = GalleryModel::new(Theme::Light, None);
        assert!(!model.toggle(Kind::ToggleButton));
        update(&mut model, GalleryMsg::Toggled(Kind::ToggleButton, true));
        assert!(model.toggle(Kind::ToggleButton));
        update(&mut model, GalleryMsg::Toggled(Kind::ToggleButton, false));
        assert!(!model.toggle(Kind::ToggleButton));
        update(&mut model, GalleryMsg::Changed(Kind::Entry, "hi".into()));
        assert_eq!(model.text(Kind::Entry), "hi");
        update(&mut model, GalleryMsg::ValueChanged(Kind::Scale, 42.0));
        assert!((model.value(Kind::Scale) - 42.0).abs() < f64::EPSILON);
        update(&mut model, GalleryMsg::Selected(Kind::DropDown, 2));
        assert_eq!(model.selection(Kind::DropDown), 2);
        assert_eq!(model.log.len(), 5, "one log line per folded message");
        assert_eq!(model.log[0], "toggled toggle_button true");
        assert_eq!(model.log[2], "changed entry hi");
    }

    #[test]
    fn a_log_line_is_one_flat_ascii_line_per_message() {
        let lines = [
            GalleryMsg::Clicked(Kind::Button).log_line(),
            GalleryMsg::Changed(Kind::Entry, "two\nlines".into()).log_line(),
            GalleryMsg::Search("a b".into()).log_line(),
            GalleryMsg::Expanded(true).log_line(),
        ];
        for line in &lines {
            assert!(!line.contains('\n'), "{line:?} must be one line");
            assert!(!line.is_empty());
        }
        assert_eq!(lines[0], "clicked button");
        assert_eq!(lines[1], "changed entry two lines");
        assert_eq!(lines[3], "expanded true");
    }

    #[test]
    fn the_sample_png_is_a_decodable_png_written_once() {
        assert_eq!(&SAMPLE_PNG[..8], b"\x89PNG\r\n\x1a\n");
        let path = sample_png_path();
        let written = std::fs::read(&path).expect("sample png is written on first ask");
        assert_eq!(written, SAMPLE_PNG);
        // Idempotent: asking twice neither panics nor changes the bytes.
        let again = sample_png_path();
        assert_eq!(again, path);
    }

    #[test]
    fn every_entry_and_container_kind_has_a_sample() {
        let model = GalleryModel::new(Theme::Light, None);
        let own = [
            Kind::Entry,
            Kind::SearchEntry,
            Kind::PasswordEntry,
            Kind::SpinButton,
            Kind::EditableLabel,
            Kind::Box,
            Kind::Grid,
            Kind::CenterBox,
            Kind::ScrolledWindow,
            Kind::Paned,
            Kind::Frame,
            Kind::Expander,
            Kind::SearchBar,
            Kind::ActionBar,
            Kind::HeaderBar,
            Kind::Notebook,
            Kind::Overlay,
            Kind::Stack,
            Kind::StackSwitcher,
            Kind::StackSidebar,
        ];
        for kind in own {
            match sample(kind, &model) {
                Sample::Own(view) => assert_eq!(view.kind, kind, "{}", kind_name(kind)),
                Sample::Within(p) => panic!("{} must be its own entry, got {p:?}", kind_name(kind)),
            }
        }
        for (child, parent) in [
            (Kind::NotebookTab, Kind::Notebook),
            (Kind::StackPage, Kind::Stack),
        ] {
            match sample(child, &model) {
                Sample::Within(got) => assert_eq!(got, parent),
                Sample::Own(_) => panic!("{} is a sub-kind", kind_name(child)),
            }
        }
    }

    /// The samples must *read* the model, or no interaction could ever change
    /// a pixel.
    ///
    /// Mutation check: make the `Entry` arm pass a literal `"Entry"` instead
    /// of `model.text(Kind::Entry)`; this test fails. Restore.
    #[test]
    fn an_entrys_sample_reflects_the_models_text() {
        let mut model = GalleryModel::new(Theme::Light, None);
        update(&mut model, GalleryMsg::Changed(Kind::Entry, "typed".into()));
        let Sample::Own(view) = sample(Kind::Entry, &model) else {
            panic!("entry is its own entry");
        };
        assert_eq!(
            view.props.str(crate::view::PropName::Text),
            Some("typed"),
            "the entry sample did not read the model"
        );
    }
}
