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

use crate::anim::clock::{Clock, MonotonicClock};
use crate::css::cascade::CompiledSheet;
use crate::css::node::Node;
use crate::css::parse::parse_stylesheet_with_base;
use crate::css::parse::{ColorScheme, Contrast, MediaEnv};
use crate::css::value::{IconRef, Rgba};
use crate::icons::IconTheme;
use crate::layout::Allocation;
use crate::layout::Rect;
use crate::text::FontDatabase;
use crate::view::app::{App, AppError, Probe};
use crate::view::builders::{
    self as w, CheckButtonExt, DropDownExt, ImageExt, InfoBarExt, LabelExt, LevelBarExt,
    MenuButtonExt, PopoverExt, ProgressBarExt, ScaleExt, ScrollbarExt, SpinnerExt, StatusbarExt,
    TextViewExt, ToggleButtonExt,
};
use crate::view::cmd::Cmd;
use crate::view::{Instance, Kind, Prop, PropName, View};
use crate::widgets::types::{
    ItemFactory, ListItem, MessageType, Orientation, RowContent, SelectionMode, Side,
};
use crate::window::{LayerSpec, Role, SurfaceSpec, Window};
use crate::{BUNDLED_ADWAITA_DARK, BUNDLED_ADWAITA_HC, BUNDLED_ADWAITA_LIGHT};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

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
    /// Render every sample whose body can be disclosed with it already
    /// disclosed — a popover shown, an expander expanded.
    ///
    /// A geometry affordance for the interaction gate, not a new interaction:
    /// a `DropDown`'s popover is hidden while closed (`PopoverC`'s own
    /// `reveal`, which lays it out at zero size), a `MenuButton`'s the same,
    /// and a collapsed `Expander`'s `content` takes no space at all, so a
    /// closed-state `--probe-points` can never say where a row — or a
    /// disclosed child — *will* be once a click opens it. `--open` builds the
    /// same tree the click produces, so the gate reads the open coordinates
    /// from the binary instead of hard-coding them, and still drives the real
    /// open by clicking.
    pub open: bool,
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
            open: false,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => opts.list = true,
                "--probe-points" => opts.probe_points = true,
                "--print-allocation" => opts.print_allocation = true,
                "--open" => opts.open = true,
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
    /// `--open`: every popover-bearing sample starts with its popover shown.
    pub open: bool,
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
            open: false,
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
                |canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
                 rect: Rect,
                 _cx: &mut crate::paint::PaintCx<'_>| {
                    let mut paint = Paint::new();
                    paint.set_color32(Color(0xFF33_D17A));
                    canvas.draw_rect(&rect.to_skia(), &paint);
                },
            )
            .width_request(64)
            .height_request(48)
            // The interaction gate reads these back off stdout. `Changed`
            // rather than a new `GalleryMsg` variant: the payload is already a
            // flat string and `log_line` folds control characters, so the
            // gate's substring matching needs nothing new.
            .on_pointer_down_with_button(|x, y, b| {
                GalleryMsg::Changed(Kind::DrawingArea, format!("down {x} {y} {b}"))
            })
            .on_pointer_motion(|x, y| {
                GalleryMsg::Changed(Kind::DrawingArea, format!("motion {x} {y} 0"))
            })
            .on_pointer_up_with_button(|x, y, b| {
                GalleryMsg::Changed(Kind::DrawingArea, format!("up {x} {y} {b}"))
            }),
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
                // `--open`: the menu's own geometry, for a gate that has to
                // know where the body lands before it clicks the button.
                .prop(PropName::Expanded, Prop::Bool(model.open))
                .child(w::label("Menu content")),
        ),
        Kind::Switch => Sample::Own(
            w::switch(model.toggle(Kind::Switch))
                .on_toggle(|on| GalleryMsg::Toggled(Kind::Switch, on)),
        ),
        Kind::DropDown => Sample::Own(
            w::drop_down(&["One", "Two", "Three"])
                .selected(model.selection(Kind::DropDown))
                // `--open`: the popover's own geometry, for a gate that has
                // to know where a row lands before it clicks the button.
                .prop(PropName::Expanded, Prop::Bool(model.open))
                .on_selected(|i| GalleryMsg::Selected(Kind::DropDown, i)),
        ),
        Kind::ColorDialogButton => Sample::Own(
            w::color_dialog_button(Rgba {
                r: 0.21,
                g: 0.52,
                b: 0.89,
                a: 1.0,
            })
            .on_value_changed(|v| GalleryMsg::ValueChanged(Kind::ColorDialogButton, v)),
        ),
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

        // ---- P6 · lists ---------------------------------------------------
        Kind::ListBox => Sample::Own(
            w::ListBoxExt::show_separators(
                w::ListBoxExt::selection_mode(
                    w::list_box([
                        w::ListBoxExt::activatable(
                            w::list_box_row(w::label("Row 0")).key(0usize),
                            true,
                        ),
                        w::ListBoxExt::activatable(
                            w::list_box_row(w::label("Row 1")).key(1usize),
                            true,
                        ),
                        w::ListBoxExt::activatable(
                            w::list_box_row(w::label("Row 2")).key(2usize),
                            true,
                        ),
                    ]),
                    SelectionMode::Single,
                ),
                true,
            )
            .on_selected(|i| GalleryMsg::Selected(Kind::ListBox, i))
            .width_request(200),
        ),
        Kind::ListBoxRow => Sample::Within(Kind::ListBox),
        Kind::FlowBox => Sample::Own(
            w::ListBoxExt::selection_mode(
                w::FlowBoxExt::max_children_per_line(
                    w::FlowBoxExt::min_children_per_line(
                        // Reconciliation: there is no separate `flow_box_child`
                        // builder -- `flow_box` already wraps each child in
                        // its own `Kind::FlowBoxChild` (its own doc comment),
                        // so the children passed here are the row content
                        // directly.
                        w::flow_box((0..6).map(|i| w::label(&format!("{i}")).key(i as usize))),
                        3,
                    ),
                    3,
                ),
                SelectionMode::Single,
            )
            .on_selected(|i| GalleryMsg::Selected(Kind::FlowBox, i))
            .width_request(200),
        ),
        Kind::FlowBoxChild => Sample::Within(Kind::FlowBox),
        Kind::ListView => Sample::Own(
            w::ListBoxExt::selection_mode(
                w::list_view(items(LIST_ROWS), row_factory()),
                SelectionMode::Single,
            )
            .selected(model.selection(Kind::ListView))
            .on_selected(|i| GalleryMsg::Selected(Kind::ListView, i))
            .width_request(200)
            .height_request(120),
        ),
        Kind::GridView => Sample::Own(
            w::GridViewExt::max_columns(
                w::GridViewExt::min_columns(w::grid_view(items(LIST_ROWS), row_factory()), 2),
                2,
            )
            .on_selected(|i| GalleryMsg::Selected(Kind::GridView, i))
            .width_request(200)
            .height_request(120),
        ),
        Kind::ColumnView => Sample::Own(
            w::ColumnViewExt::show_column_separators(
                w::ColumnViewExt::show_row_separators(
                    w::column_view(
                        items(LIST_ROWS),
                        [
                            w::ColumnViewExt::resizable(
                                w::column_view_column("Name", row_factory()),
                                true,
                            ),
                            w::ColumnViewExt::expand(
                                w::column_view_column("Value", row_factory()),
                                true,
                            ),
                        ],
                    ),
                    true,
                ),
                true,
            )
            .on_selected(|i| GalleryMsg::Selected(Kind::ColumnView, i))
            .width_request(240)
            .height_request(120),
        ),
        Kind::ColumnViewColumn => Sample::Within(Kind::ColumnView),

        // ---- P6 · menus -----------------------------------------------
        //
        // Reconciliation: the task text binds `PopoverMenu`/`PopoverMenuBar`'s
        // selection with `.on_activate(|i| ..)`, but `View::on_activate` (the
        // only `on_activate` in scope) takes a plain `Msg`, not an
        // `Fn(usize) -> Msg` -- it is `Handler::Unit`'s zero-argument setter.
        // The index-carrying setter both controllers actually fire
        // (`EventCx::fire_index(EventKind::Activate, ..)`, per
        // `popover_menu.rs`/`popover_menu_bar.rs`) is
        // `View::on_item_activated`, so both arms below call that instead.
        Kind::PopoverMenu => Sample::Own(
            w::popover_menu([
                w::PopoverMenuItemExt::accel(w::popover_menu_item("Open").key("open"), "<Ctrl>O"),
                w::PopoverMenuItemExt::accel(w::popover_menu_item("Save").key("save"), "<Ctrl>S"),
                w::popover_menu_item("Quit").key("quit"),
            ])
            .autohide(false)
            .on_item_activated(|i| GalleryMsg::Selected(Kind::PopoverMenu, i)),
        ),
        Kind::PopoverMenuBar => Sample::Own(
            w::PopoverMenuBarExt::menu(
                w::popover_menu_bar([w::popover_menu_item("File").key("file")]),
                "File",
                w::label("File menu"),
            )
            .on_item_activated(|i| GalleryMsg::Selected(Kind::PopoverMenuBar, i))
            .width_request(200),
        ),
        Kind::PopoverMenuItem => Sample::Within(Kind::PopoverMenu),

        // ---- P6 · windows -------------------------------------------------
        // The window-ish kinds are node trees like any other: the gallery
        // embeds them in the page rather than mapping four more surfaces.
        //
        // Reconciliation: `Window` has no `.title()` of its own (`WindowExt`
        // covers `titlebar`/`resizable`/`modal`/`deletable`/`decorated`/
        // `default_size`/`icon_name`, but not a plain window title) --
        // `PropName::Title` is the prop `WindowC::build` reads for it
        // (`window.rs`'s own `"Files"` fixture sets it the same way), and the
        // only setter over that prop in scope is `HeaderBarExt::title`, so
        // that is what is called here, by full path to avoid claiming
        // `HeaderBar`'s trait for this whole file.
        Kind::Window => Sample::Own(
            w::HeaderBarExt::title(w::window(w::label("Window content")), "Window")
                .width_request(220)
                .height_request(80),
        ),
        Kind::ShortcutsWindow => Sample::Own(
            w::ShortcutsSectionExt::view_name(
                w::ShortcutsSectionExt::section_name(
                    w::shortcuts_window([w::label("Ctrl+Q — Quit")]),
                    "general",
                ),
                "main",
            )
            .width_request(220)
            .height_request(80),
        ),
        Kind::AboutDialog => Sample::Own(
            w::AboutDialogExt::comments(
                w::AboutDialogExt::version(w::about_dialog("icedtea"), "0.1.0"),
                "A pure-Rust GTK-themed toolkit",
            )
            .width_request(220)
            .height_request(100),
        ),
        Kind::AlertDialog => Sample::Own(
            w::AlertDialogExt::buttons(
                w::AlertDialogExt::detail(
                    w::alert_dialog("Delete everything?"),
                    "This cannot be undone.",
                ),
                ["Cancel", "Delete"],
            )
            .on_response(|i| GalleryMsg::Selected(Kind::AlertDialog, i))
            .width_request(240),
        ),
    }
}

/// The rows every list-ish sample shows. Ten, so `ListView` has more rows than
/// fit its 120 px viewport and the scroll interaction actually recycles.
const LIST_ROWS: &[&str] = &[
    "Row 0", "Row 1", "Row 2", "Row 3", "Row 4", "Row 5", "Row 6", "Row 7", "Row 8", "Row 9",
];

/// A list model from plain strings.
///
/// Reconciliation: the task's "Produces" section assumed `ListItem::text`,
/// but the real constructor is `ListItem::new(id, text)` (§4.3/D7's `id` is
/// the stable identity a keyed row uses) -- the row's own index doubles as
/// its id here, since these are static fixtures with no reordering.
#[must_use]
pub fn items(labels: &[&str]) -> Rc<[ListItem]> {
    labels
        .iter()
        .enumerate()
        .map(|(i, t)| ListItem::new(i as u64, t))
        .collect()
}

/// The factory every list-ish sample binds its rows with: one label per item.
///
/// Reconciliation: the task's "Produces" section assumed the `factory`
/// parameter of `list_view`/`grid_view`/`column_view_column` was
/// `Rc<dyn Fn(usize, &ListItem) -> View<Msg>>`, but the real type is
/// `crate::widgets::types::ItemFactory`, wrapping `Fn(usize, &ListItem) ->
/// RowContent` -- deliberately `Msg`-free (`RowContent`'s own doc comment:
/// "rebinding a pooled row must not allocate a view subtree"), so this binds
/// `RowContent::from_label` instead of building a `View`.
fn row_factory() -> ItemFactory {
    ItemFactory::new(|_index, item: &ListItem| RowContent::from_label(&item.text))
}

/// Every kind that gets its own framed entry, in `Kind::all()` order.
#[must_use]
pub fn own_kinds() -> Vec<Kind> {
    Kind::all()
        .iter()
        .copied()
        .filter(|&k| sample_shape(k) == SampleShape::Own)
        .collect()
}

/// The gallery's whole view.
///
/// In `--widget` mode it is the bare sample, at the origin, so the interaction
/// gate's coordinates are the widget's own. Otherwise it is one vertical box of
/// frames, one per own kind, in `Kind::all()` order, built by *iterating*
/// `Kind::all()` — that iteration is what makes a missing entry impossible.
#[must_use]
pub fn page(model: &GalleryModel) -> View<GalleryMsg> {
    if let Some(kind) = model.only {
        return match sample(kind, model) {
            Sample::Own(view) => view
                .halign(crate::layout::Align::Start)
                .valign(crate::layout::Align::Start),
            // Unreachable through `Options::parse`, which rejects a sub-kind.
            Sample::Within(parent) => w::label(&format!(
                "{} is rendered inside {}",
                kind_name(kind),
                kind_name(parent)
            )),
        };
    }
    let frames = own_kinds().into_iter().map(|kind| {
        let Sample::Own(view) = sample(kind, model) else {
            unreachable!("own_kinds() filtered to Sample::Own");
        };
        crate::widgets::button::ButtonExt::label(w::frame(view), kind_name(kind))
            .key(kind_name(kind))
            .halign(crate::layout::Align::Start)
    });
    w::BoxExt::spacing(w::box_(Orientation::Vertical, frames), 12)
        .margin(12, 12, 12, 12)
        .class("gallery")
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

/// One derived probe point, in page coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    /// The widget's [`kind_name`].
    pub widget: &'static str,
    /// The CSS node name this point sits on, indexed when repeated.
    pub label: String,
    /// Centre of that node's border box.
    pub x: i32,
    /// Centre of that node's border box.
    pub y: i32,
}

/// The compiled sheet `opts` asks for: a file used *whole*, or the bundled
/// sheet compiled under the theme's own `@media` environment.
#[must_use]
pub fn compile_sheet(opts: &Options) -> CompiledSheet {
    let env = opts.theme.media_env();
    // `--scroll`'s one caller-visible rule: `scrolled_page` shifts the page
    // by adding this class, never by `View::margin` (a prop no controller
    // in this tree reads back into layout — see `scrolled_page`'s doc).
    // CSS `margin`, unlike that prop, is the real, taffy-consumed box model,
    // so a one-off class carries the offset the rest of the sheet cannot
    // know ahead of time. Relative to the flat page's own margin (`page`'s
    // `View::margin(12, ..)` call is that same inert prop, so the flat
    // page's real top margin is 0) rather than to GTK's nominal 12px, so the
    // shift is exactly `--scroll`, not `--scroll` plus whatever the prop
    // would have contributed had it worked.
    let scroll_rule = format!(".gallery-scroll {{ margin-top: {}px; }}", -opts.scroll);
    match &opts.theme_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(css) => CompiledSheet::compile_with_env(
                &parse_stylesheet_with_base(&format!("{css}\n{scroll_rule}"), path.parent()),
                &env,
            ),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "cannot read --theme-file; using the bundled sheet");
                CompiledSheet::compile_with_env(
                    &parse_stylesheet_with_base(
                        &format!("{}\n{scroll_rule}", opts.theme.sheet()),
                        None,
                    ),
                    &env,
                )
            }
        },
        None => CompiledSheet::compile_with_env(
            &parse_stylesheet_with_base(&format!("{}\n{scroll_rule}", opts.theme.sheet()), None),
            &env,
        ),
    }
}

/// Build, style and lay the page out with no compositor.
///
/// # Errors
///
/// Whatever [`App::probe`] returns.
pub fn build(opts: &Options) -> Result<Probe<GalleryMsg>, AppError> {
    let mut model = GalleryModel::new(opts.theme, opts.widget);
    model.scroll = opts.scroll;
    model.open = opts.open;
    // `--open` discloses every retained body, not only a popover: a collapsed
    // `Expander` gives its `content` no space, so the gate needs the expanded
    // tree to know where the disclosed child lands.
    model.expanded = opts.open;
    let app = App::new(model, update, scrolled_page);
    app.probe(
        opts.size,
        compile_sheet(opts),
        FontDatabase::new(),
        IconTheme::from_env(),
        Rc::new(MonotonicClock::new()) as Rc<dyn Clock>,
    )
}

/// [`page`] with `model.scroll` applied as a negative top margin.
///
/// Shifting the page node itself, rather than tracking an offset beside it,
/// is what lets the probe points be read straight off the laid-out tree: the
/// scrolled tree *is* the tree. A plain `fn`, because `App::new` takes a
/// function pointer, not a closure.
///
/// Reconciliation: the plan reached for `View::margin`, but that prop has no
/// reader anywhere in the tree this part's controllers form — `GenericC`
/// (what `Kind::Box` reconciles to) forwards every prop `apply_universal`
/// does not claim (`Classes`/`Focusable`/`Id`/`Sensitive`/`Checked`/
/// `Indeterminate`/`Selected`) straight to its `Label`/`Text` check and drops
/// it there, so it never reaches a taffy box. CSS `margin`, resolved and
/// consumed by `write_styles`/`layout_tree` on every kind including `Box`,
/// is; `--scroll`'s effect rides that path instead, through the
/// `gallery-scroll` class [`compile_sheet`] gives a rule to.
pub fn scrolled_page(model: &GalleryModel) -> View<GalleryMsg> {
    let built = page(model);
    if model.scroll == 0 || model.only.is_some() {
        built
    } else {
        built.class("gallery-scroll")
    }
}

/// The `(widget name, instance)` pairs the printing modes and the gates walk.
///
/// In `--widget` mode the root instance *is* the widget. Otherwise the root is
/// the page box and its children are the frames, each holding exactly one
/// sample, in `own_kinds()` order.
#[must_use]
pub fn entry_instances<'a>(
    opts: &Options,
    probe: &'a Probe<GalleryMsg>,
) -> Vec<(&'static str, &'a Instance<GalleryMsg>)> {
    let Some(root) = probe.instances().first() else {
        return Vec::new();
    };
    if let Some(kind) = opts.widget {
        return vec![(kind_name(kind), root)];
    }
    own_kinds()
        .into_iter()
        .zip(root.children.iter())
        .filter_map(|(kind, frame)| Some((kind_name(kind), frame.children.first()?)))
        .collect()
}

/// Derive `instance`'s probe points: `"root"`, then every descendant node,
/// labelled by its CSS node name and indexed when that name repeats.
///
/// Every descendant with an allocation gets a point, even a collapsed one —
/// `Switch`'s two `image` subnodes are legitimate zero-size chrome in every
/// bundled Adwaita variant (upstream GTK never paints them either) and are
/// still real nodes a caller may need to name and locate. Only a node with
/// no allocation at all (not laid out, or not in this tree) is skipped.
#[must_use]
pub fn probe_points_of(
    instance: &Instance<GalleryMsg>,
    widget: &'static str,
    probe: &Probe<GalleryMsg>,
) -> Vec<ProbePoint> {
    let mut points = Vec::new();
    if let Some(alloc) = probe.allocation(&instance.node) {
        points.push(centre(widget, "root".to_string(), &alloc));
    }
    let descendants: Vec<Node> = instance.node.descendants().collect();
    let mut counts: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        *counts.entry(node.name()).or_default() += 1;
    }
    let mut seen: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        let name = node.name();
        let index = seen.entry(name.clone()).or_default();
        let label = if counts.get(&name).copied().unwrap_or(0) > 1 {
            format!("{name}{index}")
        } else {
            name.to_string()
        };
        *index += 1;
        if let Some(alloc) = probe.allocation(node) {
            points.push(centre(widget, label, &alloc));
        }
    }
    points
}

/// The centre of a border box, floored to the pixel below.
///
/// `f32::floor` before the cast, not a bare `as i32` (which truncates toward
/// zero) or `f32::round` (ties away from zero): both move a positive and a
/// negative half-pixel centre in opposite directions, so shifting a box by
/// an integer number of pixels would not always shift its rounded centre by
/// that same integer. `floor(v - k) == floor(v) - k` for every real `v` and
/// integer `k`, so a scrolled copy of a box centres exactly `--scroll`
/// pixels from its unscrolled original, which is what
/// `scroll_shifts_every_probe_point_by_exactly_that_many_pixels` checks.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn centre(widget: &'static str, label: String, alloc: &Allocation) -> ProbePoint {
    let r = alloc.border_box;
    ProbePoint {
        widget,
        label,
        x: (r.x + r.width / 2.0).floor() as i32,
        y: (r.y + r.height / 2.0).floor() as i32,
    }
}

/// `<widget> <label> <x> <y>`, one per line.
///
/// # Errors
///
/// Whatever [`build`] returns.
pub fn print_probe_points(opts: &Options) -> Result<(), AppError> {
    let probe = build(opts)?;
    for (widget, instance) in entry_instances(opts, &probe) {
        for point in probe_points_of(instance, widget, &probe) {
            println!("{} {} {} {}", point.widget, point.label, point.x, point.y);
        }
    }
    Ok(())
}

/// `<widget> <x> <y> <width> <height>`, one per line: each entry's border box.
///
/// # Errors
///
/// Whatever [`build`] returns.
pub fn print_allocations(opts: &Options) -> Result<(), AppError> {
    let probe = build(opts)?;
    for (widget, instance) in entry_instances(opts, &probe) {
        if let Some(alloc) = probe.allocation(&instance.node) {
            let r = alloc.border_box;
            println!("{widget} {} {} {} {}", r.x, r.y, r.width, r.height);
        }
    }
    Ok(())
}

/// Map the gallery as a layer-shell overlay and run the app loop.
///
/// A layer surface rather than an xdg-toplevel: probe coordinates are output
/// coordinates, and only the layer role has a compositor-independent origin
/// (anchored top-left, margin 0 — the M2 `themed-button` precedent). Keyboard
/// interactivity is exclusive so the typing and Tab interactions have a
/// focused keyboard.
///
/// Reconciliation: the task text's `SurfaceSpec::Layer { namespace, layer,
/// anchor, size, margin, exclusive_zone, keyboard_interactivity, scale }`
/// does not match this crate's real shape -- `SurfaceSpec` is a struct with
/// `role: Role` (`Role::Toplevel` or `Role::Layer(LayerSpec)`), `size`,
/// `title` and `app_id`, and the layer's namespace is `spec.title` (see
/// `Layer::create`'s `namespace: &str` parameter, which `Window::open` feeds
/// `&spec.title`). There is no `scale` field on either type: the live scale
/// arrives over `InputEvent::ScaleChanged` and `App::run` already applies it
/// to the icon theme it takes from the window, so `opts.scale` (meaningful
/// only for the headless `--probe-points`/`--print-allocation` geometry
/// paths `build` already serves) is not threaded through here.
pub fn run(opts: &Options) -> Result<(), AppError> {
    let mut model = GalleryModel::new(opts.theme, opts.widget);
    model.scroll = opts.scroll;
    model.open = opts.open;
    // `--open` discloses every retained body, not only a popover: a collapsed
    // `Expander` gives its `content` no space, so the gate needs the expanded
    // tree to know where the disclosed child lands.
    model.expanded = opts.open;
    let spec = SurfaceSpec {
        role: Role::Layer(LayerSpec {
            layer: zwlr_layer_shell_v1::Layer::Overlay,
            anchor: zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left,
            margin: [0, 0, 0, 0],
            exclusive_zone: -1,
            keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        }),
        size: opts.size,
        title: "icedtea-gallery".to_string(),
        app_id: "org.icedtea.Gallery".to_string(),
    };
    let window =
        Window::open(spec, compile_sheet(opts), FontDatabase::new()).map_err(AppError::Surface)?;
    App::new(model, update, scrolled_page).run(window)
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

    /// The completeness assertion the whole M3 gate rests on.
    ///
    /// Mutation check: add a `Sample::Within(Kind::Box)` arm for `Kind::Label`;
    /// this test fails naming `label`. Restore.
    #[test]
    fn sample_is_total_and_every_sub_kind_names_a_real_parent() {
        let model = GalleryModel::new(Theme::Light, None);
        for &kind in Kind::all() {
            match sample(kind, &model) {
                Sample::Own(view) => {
                    assert_eq!(view.kind, kind, "{}'s sample root", kind_name(kind));
                    assert_eq!(sample_shape(kind), SampleShape::Own, "{}", kind_name(kind));
                }
                Sample::Within(parent) => {
                    assert_ne!(parent, kind, "{} cannot contain itself", kind_name(kind));
                    assert_eq!(
                        sample_shape(kind),
                        SampleShape::Within(parent),
                        "sample() and sample_shape() disagree about {}",
                        kind_name(kind)
                    );
                    assert!(
                        matches!(sample(parent, &model), Sample::Own(_)),
                        "{}'s parent {} must be its own entry",
                        kind_name(kind),
                        kind_name(parent)
                    );
                }
            }
        }
    }

    #[test]
    fn the_page_holds_one_frame_per_own_kind_in_kind_order() {
        let model = GalleryModel::new(Theme::Light, None);
        let page = page(&model);
        let expected = own_kinds();
        assert_eq!(
            page.children.len(),
            expected.len(),
            "one frame per own kind"
        );
        for (frame, kind) in page.children.iter().zip(&expected) {
            assert_eq!(
                frame.kind,
                Kind::Frame,
                "{} is not framed",
                kind_name(*kind)
            );
            assert_eq!(
                frame.props.str(crate::view::PropName::Label),
                Some(kind_name(*kind)),
                "the frame's label is the widget's name"
            );
            assert_eq!(frame.children.len(), 1);
            assert_eq!(frame.children[0].kind, *kind);
        }
    }

    #[test]
    fn the_page_in_widget_mode_is_the_bare_sample() {
        let model = GalleryModel::new(Theme::Light, Some(Kind::CheckButton));
        let page = page(&model);
        assert_eq!(page.kind, Kind::CheckButton, "no frame, no siblings");
    }

    #[test]
    fn scrolling_shifts_the_page_and_never_the_isolated_widget() {
        let model = GalleryModel::new(Theme::Light, None);
        let page = page(&model);
        assert_eq!(page.kind, Kind::Box, "the page is one vertical box");
    }

    #[test]
    fn every_widget_exposes_a_root_probe_point_inside_its_own_allocation() {
        let opts = Options::parse(vec![]).unwrap();
        let probe = build(&opts).expect("the page lays out headlessly");
        let entries = entry_instances(&opts, &probe);
        assert_eq!(entries.len(), own_kinds().len());
        for (widget, instance) in entries {
            let points = probe_points_of(instance, widget, &probe);
            assert!(!points.is_empty(), "{widget} has no probe points");
            assert_eq!(
                points[0].label, "root",
                "{widget}'s first point is its root"
            );
            let mut labels: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
            labels.sort_unstable();
            let before = labels.len();
            labels.dedup();
            assert_eq!(before, labels.len(), "{widget} has duplicate probe labels");
            for point in &points {
                assert!(
                    point.x >= 0 && point.y >= 0,
                    "{widget}/{} is off the surface at ({}, {})",
                    point.label,
                    point.x,
                    point.y
                );
            }
        }
    }

    /// Repetition gets indices, uniqueness does not — GTK's own `tab0`/`row0`
    /// convention, derived rather than hard-coded.
    ///
    /// Mutation check: always append the index; `check_button`'s point becomes
    /// `check0` and this test fails. Restore.
    #[test]
    fn repeated_node_names_are_indexed_and_unique_ones_are_not() {
        let opts = Options::parse(vec!["--widget".into(), "switch".into()]).unwrap();
        let probe = build(&opts).expect("the switch lays out");
        let entries = entry_instances(&opts, &probe);
        let points = probe_points_of(entries[0].1, entries[0].0, &probe);
        let labels: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
        assert!(labels.contains(&"root"));
        assert!(
            labels.contains(&"slider"),
            "switch > slider is unique and keeps its bare name: {labels:?}"
        );
        assert!(
            labels.contains(&"image0") && labels.contains(&"image1"),
            "switch has two image subnodes, so both are indexed: {labels:?}"
        );
    }

    #[test]
    fn scroll_shifts_every_probe_point_by_exactly_that_many_pixels() {
        let flat = Options::parse(vec![]).unwrap();
        let scrolled = Options::parse(vec!["--scroll".into(), "100".into()]).unwrap();
        let a = build(&flat).unwrap();
        let b = build(&scrolled).unwrap();
        let (wa, ia) = entry_instances(&flat, &a)[0];
        let (_wb, ib) = entry_instances(&scrolled, &b)[0];
        let pa = probe_points_of(ia, wa, &a);
        let pb = probe_points_of(ib, wa, &b);
        assert_eq!(pa[0].x, pb[0].x, "scrolling never moves x");
        assert_eq!(pa[0].y - 100, pb[0].y, "scrolling moves y by --scroll");
    }
}
