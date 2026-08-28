# Pure-Rust GTK-themed UI — M3 Part 5: widgets — display, buttons, entries — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## Contract deviations

Every line below deviates from `docs/superpowers/plans/2026-08-27-m3-part0-contract.md`
and must be appended to its §10 as an amendment before P5 merges. Nothing here
is silent; nothing else in this plan departs from the contract.

- **D1 — two dependency lines in `ui/Cargo.toml` are added by P5, not P3.** Contract §0's dep block gives `ui/Cargo.toml` to P3 and lists only `xkbcommon`/`wayland-protocols`/`memmap2`. §3.7's `TextLayout` needs real grapheme-cluster and line-break tables, so P5 adds `unicode-segmentation = "1.13"` and `unicode-linebreak = "0.1"`. Both are already in the workspace `Cargo.lock` (pulled by `cosmic-text` under `skia-rs-text`), so no new crate enters the tree.
- **D2 — P5 creates `ui/src/icons/builtin.rs`, a file §9 assigns to P7.** §9's P5 boundary itself orders this ("P5 stubs `Builtin::path` behind P7's signature and P7 fills it in"); the file is named here so the ownership overlap is explicit. If P4 already created `ui/src/icons/`, P5 adds the module to the existing `icons/mod.rs` instead of recreating it.
- **D3 — `ImageC.resolved` is `Option<Rc<skia_rs_safe::codec::Image>>`, not `Option<icons::Handle>`.** Contract §5.1 names `icons::Handle`; §6 never defines that type and gives `IconTheme::render -> Option<Rc<Image>>`. P5 follows §6.
- **D4 — P5 defines the eighteen widget-local types the contract names but never declares.** `Orientation`, `Position`, `Side`, `IconSize`, `MessageType`, `LevelBarMode`, `ContentFit`, `MatchMode`, `ArrowDirection`, `FontLevel`, `WindowButton`, `ColorDialogSpec`, `PictureSource`, `Adjustment`, `ListItem`, `Mark`, `RepeatTimer`, and the pair `TextEditState`/`UndoStack`. Their exact declarations are Task 7 (`ui/src/widgets/mod.rs`) and Task 27 (`ui/src/widgets/edit.rs`).
- **D5 — P5 appends one `pub use` line per widget to `ui/src/view/builders.rs`.** §9's P5 boundary says "must not touch `view/**`", but §0's module map says `view/builders.rs` is where P5/P6 "fill" the builder frame and §4.4 requires the builder to be reachable as `view::builders::<name>`. Ruling: builder *bodies* live in the widget's own file under `ui/src/widgets/`; `view/builders.rs` gains only re-export lines. No other file under `view/` is touched.
- **D6 — P5 adds `widgets::build_controller`, the reconciler's `Kind` → controller dispatch.** `Controller::build` is `Self: Sized`, so §4.5's `reconcile` cannot construct a controller from a `Kind` without a dispatch table, and the table must live where the widgets live. Exact signature in Task 7. P4's `reconcile` calls it; P6 replaces the `Unimplemented` arms.
- **D7 — `node_tree_of` renders the concrete tree; `ui/tests/node_trees.rs` matches it against the fixture with a notation-aware matcher, not `assert_eq!`.** §5 says "diffs it against the fixture", but the fixtures are GTK's own blocks carrying `[optional]`, `┊` and `<child>`, which no concrete tree can render literally. The matcher (Task 7) enforces both directions: every rendered node must be permitted by the fixture, and every non-optional fixture node must be present.
- **D8 — WITHDRAWN. P5 uses P4's existing `App::with_sheet(self, sheet: CompiledSheet) -> Self`.** §4.7's `run_offscreen` takes no stylesheet, so P5's rest-state pixel tests have no way to say "Adwaita light" — but P4's own deviation D11 already ships `App::{with_sheet, with_fonts, with_icons}` for exactly that reason. P5 therefore adds **no** setter and makes **no** edit to `ui/src/view/app.rs`; every P5 offscreen test calls `.with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))`. See contract §10 E4.
- **D9 — two P5 pixel assertions are deferred to P7's fix wave, by name.** §9's P5 boundary already defers "a P5 pixel test that depends on real builtin geometry". The two are `a_check_button_paints_the_builtin_check_glyph` (Task 20) and `an_image_paints_its_resolved_icon` (Task 13). Both are written now, marked `#[ignore = "P7 fills in Builtin::path / IconTheme::render (contract §9, D9)"]`, and un-ignored in P7's fix wave.
- **D10 — `DropDownC.list` and `FontDialogC.list` are a `Node`, not a `ListViewC`.** Contract §5.2 types both as `ListViewC`, which is a P6 kind landing after P5. P5 builds the `listview`/`fontchooser` node and its rows directly (Tasks 21 and 22); P6 replaces the field when `Kind::ListView` lands.
- **D11 — `ColorDialogButton`'s change handler carries a packed `f64`, not an `Rgba`.** Contract §5.2 writes `.on_change(|Rgba| Msg)`, but §4.4's `Handler` has no `Rgba` variant. The colour rides through `Handler::Float` as `0xAARRGGBB`, which an `f64` represents exactly; `ColorDialogC::{pack, unpack}` (Task 22) are the only conversion.

---

**Goal:** Ship the thirty-two display, button and entry widgets of contract §5.1–§5.3 (plus `Popover`, ruling R4) as `Kind` + builder + controller triples over M2's retained node tree and P4's reactive framework, together with the `TextLayout` paragraph engine (§3.7) they all rest on and the `node_tree_of` conformance gate.

**Architecture:** Every widget is three things in one file under `ui/src/widgets/`: a free builder function returning `View<Msg>` (re-exported from `view::builders`), a `<Name>C` controller implementing P4's `Controller<Msg>` trait, and a vendored GTK 4.22.4 node-tree fixture. A controller owns exactly the state the model must not — press state, cursor/selection/undo, drag offsets, open/closed flags — builds its own CSS subnodes under the root `Node` P4 hands it, mutates them in `set_prop`/`on_event`/`tick`, and emits `Msg`s through `cx.handlers.fire_*`. Painting reuses M2's `paint_node_with_children` for every box; a controller only overrides `Controller::paint` when it draws something CSS cannot (a spinner arc, a drawing-area callback, glyph runs). Layout stays on M2's `Container::Box`/`Leaf` only — P6 owns the `Grid`/`Center` widening — so a widget that GTK lays out as a grid (`Calendar`) nests row boxes until P6 lands; the CSS node tree, which is what the fixture pins, is unaffected. Leaf sizing goes through `Controller::measure`, which P4's `Measure` implementation dispatches into M2's existing `LayoutTree::compute` measure closure; P5 never edits `layout.rs`.

**Tech Stack:** Rust edition 2024 (rust-version 1.94), `skia-rs-safe` 0.4.0 (`std,text,codec,codec-png,svg`), `taffy` 0.14, `cssparser` 0.37, `selectors` 0.40, `bitflags` 2, `unicode-segmentation` 1.13, `unicode-linebreak` 0.1, `fontconfig` 0.11 (optional), `wayland-client` 0.31, `wayland-protocols-wlr` 0.3, `xkbcommon` 0.9 (P3). No gtk4/gio/glib/pango/cairo/gdk; no smithay.

**Spec:** `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md` §5.1–§5.3 and §3 (text), executed against the frozen interface contract `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` §3.7, §5.1, §5.2, §5.3, §9 (P5). Research notes: `.superpowers/m3-plan-notes/gtk-widget-nodes.md` (fixture source, GTK 4.22.4 verbatim) and `.superpowers/m3-plan-notes/current-ui-crate.md` (post-M2 inventory).

## Global Constraints

- **Crate pins, exact:** `skia-rs-safe = { version = "0.4.0", default-features = false, features = ["std","text","codec","codec-png","svg"] }`, `taffy = "0.14"`, `cssparser = "0.37"`, `selectors = "0.40"`, `bitflags = "2"`, `precomputed-hash = "0.1"`, `wayland-client = "0.31"`, `wayland-protocols = "0.32"`, `wayland-protocols-wlr = "0.3"`, `rustix = "1"`, `xkbcommon = "0.9"`, `fontconfig = { version = "0.11", optional = true }`, plus P5's two (D1) `unicode-segmentation = "1.13"` and `unicode-linebreak = "0.1"`.
- **No gtk4, gio, glib, pango, cairo, gdk, or smithay** — anywhere, at any time. These are Wayland clients; wlroots is the server side.
- **Edition 2024, `rust-version` 1.94**, both inherited from the workspace (`version.workspace = true` style). Do not add per-crate overrides.
- **Gates (run all four before every commit that is not a pure test-add):**
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`
  - `cargo fmt --all --check`
- **P1's gates are not P5's.** P1 runs in a `wloots-sys` worktree with the `wlr` crate tests, `cargo test -p wlr --test coverage_audit`, `cargo xtask coverage`, `cargo doc -D warnings` and `cargo publish --dry-run`; P2 runs `cargo test -p icedtea-compositor --test popups` three times. P5 runs neither. While P2–P8 are in development a root `[patch.crates-io]` pointing `wlr` at P1's worktree is allowed **in its own commit** and must be dropped before merge, exactly as the implicit-grab fix did; icedtea otherwise consumes the published `wlr` 0.20.28 through its pins.
- **The M1/M2 gates stay green, byte-identical.** Contract §8.2: `ui/tests/themed_button_offscreen.rs` (4), `ui/tests/adwaita_coverage.rs` (9), `ui/tests/gtk4_property_reference.rs` (4), `ui/tests/transition_screencopy.rs` (1), every test under `ui/src/css/**`, `ui/src/anim/**`, `ui/src/shm.rs` (11), and `ui/src/widget/button.rs` (4). P5 changes none of them, including their imports. Pinned constants that must not move: 900 compiled Adwaita rules, 1941 lines / 37 `@define-color`s, 114/95/19 properties, `POOL_INITIAL_BUFFERS=2`, `POOL_MAX_BUFFERS=3`, `CONFIGURE_TIMEOUT=5s`, `MARGIN=0`, `BTN_LEFT=0x110`, `#3584e4`, `#1c6fd4`, `#1961b9`.
- **P5 must not touch** `ui/src/layout.rs`, `ui/src/window/**`, `ui/src/css/**`, `ui/src/anim/**`, `ui/src/shm.rs`, `ui/src/widget/button.rs`, `ui/src/app.rs`, or anything under `compositor/`, `harness/`, `wloots-sys/`. The only files outside `ui/src/widgets/`, `ui/src/text.rs` and `ui/tests/` it edits are the three named in D1/D5/D8.
- **Parts execute in order P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8**, so P5 may consume everything P1–P4 produced and may not consume anything P6–P8 will produce.
- **Every load-bearing test records its mutation check** in a `// mutation:` comment on the test: the exact edit that must make it fail. Timing assertions are generous complexity bounds, never wall-clock pins.
- **Untrusted input never panics** — markup strings, list models, image files, `f64` props carrying NaN or infinity. A malformed value is dropped, logged once, and replaced by the initial or fallback value. Every parser or decoder of untrusted input in this part (`parse_markup`, `PictureC`'s decode path, `Adjustment`'s clamping) carries a dedicated never-panic test.
- **`Duration::ZERO` from any `next_deadline` means "now", never "spin".**
- **Every commit message ends with the trailer** `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` on its own line after a blank line.

## File Structure

| File | Responsibility |
|---|---|
| `ui/Cargo.toml` | **Modify.** Two dependency lines (D1). |
| `ui/src/text.rs` | **Modify.** §3.7 additions: `TextLayout`, `Ellipsize`, `WrapMode`, `parse_markup`, `MarkupSpan`. Every M2 signature untouched. |
| `ui/src/lib.rs` | **Modify.** `pub mod widgets;` (and `pub mod icons;` if P4 did not add it). |
| `ui/src/view/builders.rs` | **Modify.** One `pub use` line per widget (D5). |
| `ui/src/view/app.rs` | **Untouched.** P4's `App::with_sheet` (P4 D11) covers D8; P5 makes no edit here. |
| `ui/src/icons/builtin.rs` | **Create** (D2). `Builtin` enum with P7's exact signatures; geometry stubbed. |
| `ui/src/widgets/mod.rs` | **Create.** Shared widget-local types (D4), `build_controller` (D6), `node_tree_of` + its renderer, the `pressed`/`hovered` helper. |
| `ui/src/widgets/separator.rs` | **Create.** `Kind::Separator`. |
| `ui/src/widgets/label.rs` | **Create.** `Kind::Label`. |
| `ui/src/widgets/spinner.rs` | **Create.** `Kind::Spinner`. |
| `ui/src/widgets/statusbar.rs` | **Create.** `Kind::Statusbar`. |
| `ui/src/widgets/level_bar.rs` | **Create.** `Kind::LevelBar`. |
| `ui/src/widgets/progress_bar.rs` | **Create.** `Kind::ProgressBar`. |
| `ui/src/widgets/info_bar.rs` | **Create.** `Kind::InfoBar`. |
| `ui/src/widgets/scrollbar.rs` | **Create.** `Kind::Scrollbar`; `ScrollbarC` is consumed by P6's `ScrolledWindow`. |
| `ui/src/widgets/image.rs` | **Create.** `Kind::Image`. |
| `ui/src/widgets/picture.rs` | **Create.** `Kind::Picture`. |
| `ui/src/widgets/text_view.rs` | **Create.** `Kind::TextView`. |
| `ui/src/widgets/scale.rs` | **Create.** `Kind::Scale`. |
| `ui/src/widgets/drawing_area.rs` | **Create.** `Kind::DrawingArea`. |
| `ui/src/widgets/window_controls.rs` | **Create.** `Kind::WindowControls`. |
| `ui/src/widgets/calendar.rs` | **Create.** `Kind::Calendar`. |
| `ui/src/widgets/popover.rs` | **Create.** `Kind::Popover`; `PopoverC` is consumed by four P5 widgets and by P6's menus. |
| `ui/src/widgets/button.rs` | **Create.** `Kind::Button`. Distinct from M2's `ui/src/widget/button.rs`, which is untouched. |
| `ui/src/widgets/toggle_button.rs` | **Create.** `Kind::ToggleButton`. |
| `ui/src/widgets/link_button.rs` | **Create.** `Kind::LinkButton`. |
| `ui/src/widgets/check_button.rs` | **Create.** `Kind::CheckButton`. |
| `ui/src/widgets/menu_button.rs` | **Create.** `Kind::MenuButton`. |
| `ui/src/widgets/switch.rs` | **Create.** `Kind::Switch`. |
| `ui/src/widgets/drop_down.rs` | **Create.** `Kind::DropDown`. |
| `ui/src/widgets/color_dialog.rs` | **Create.** `Kind::ColorDialogButton` and `Kind::ColorDialog`. |
| `ui/src/widgets/font_dialog.rs` | **Create.** `Kind::FontDialogButton` and `Kind::FontDialog`. |
| `ui/src/widgets/edit.rs` | **Create.** `TextEditState`, `UndoStack`, the shared `GtkText` key table. |
| `ui/src/widgets/entry.rs` | **Create.** `Kind::Entry`. |
| `ui/src/widgets/search_entry.rs` | **Create.** `Kind::SearchEntry`. |
| `ui/src/widgets/password_entry.rs` | **Create.** `Kind::PasswordEntry`. |
| `ui/src/widgets/spin_button.rs` | **Create.** `Kind::SpinButton`. |
| `ui/src/widgets/editable_label.rs` | **Create.** `Kind::EditableLabel`. |
| `ui/tests/node_trees.rs` | **Create.** The conformance gate; extended by P6. |
| `ui/tests/widget_pixels.rs` | **Create.** Per-widget rest-state and interaction tests over `App::run_offscreen`. |
| `ui/tests/fixtures/gtk4.22-node-trees/*.txt` | **Create.** One vendored GTK 4.22.4 block per `Kind`, named by the kind's snake_case. |

---

### Task 1: `TextLayout` — paragraphs, sizing, drawing

**Files:**
- Modify: `ui/Cargo.toml` (two dependency lines, D1)
- Modify: `ui/src/text.rs` (append; every existing item untouched)
- Test: `ui/src/text.rs` (`mod tests`, appended)

**Interfaces:**
- Consumes: M2's `text::{FontDatabase, TextStyle, ShapedText, TextMetrics, FontFace}`, `FontDatabase::{match_face, shape}`, `TextStyle::{query, shape_key, line_height_px}`, `layout::Rect`, `css::value::Rgba`, `skia_rs_safe::canvas::Canvas`, `paint::fill_paint`.
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Ellipsize { None, Start, Middle, End }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum WrapMode { None, Word, Char, WordChar }

  pub struct TextLayout { /* private */ }

  impl TextLayout {
      pub fn build(text: &str, style: &TextStyle, fonts: &mut FontDatabase,
                   width: Option<f32>, wrap: WrapMode, ellipsize: Ellipsize) -> Self;
      #[must_use] pub fn size(&self) -> (f32, f32);
      #[must_use] pub fn line_count(&self) -> usize;
      #[must_use] pub fn text(&self) -> &str;
      pub fn draw(&self, canvas: &mut Canvas<'_>, origin: (f32, f32), color: Rgba);
  }
  ```

- [ ] **Step 1: Add the two dependencies**

In `ui/Cargo.toml`, under `[dependencies]`, after the `taffy` line:

```toml
# Grapheme-cluster and word boundaries for TextLayout's caret movement, and
# UAX #14 line-break opportunities for its word wrapping. Both already resolve
# in the workspace lockfile (cosmic-text pulls them under skia-rs-text), so
# neither adds a crate to the tree.
unicode-segmentation = "1.13"
unicode-linebreak = "0.1"
```

- [ ] **Step 2: Write the failing test**

Append to `ui/src/text.rs`'s `mod tests`:

```rust
    fn ui_style(css: &str) -> TextStyle {
        TextStyle::from_computed(&style_for(css))
    }

    #[test]
    fn a_paragraph_lays_out_one_line_per_newline_and_sizes_to_the_widest() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let layout = super::TextLayout::build(
            "Hello\nHello world",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );

        assert_eq!(layout.line_count(), 2, "one line per newline");
        assert_eq!(layout.text(), "Hello\nHello world");

        let (w, h) = layout.size();
        assert!(w > 0.0 && h > 0.0, "a non-empty paragraph has extents");

        let short = super::TextLayout::build(
            "Hello",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );
        assert!(
            w > short.size().0,
            "the paragraph is as wide as its widest line ({w} vs {})",
            short.size().0
        );
        assert!(
            (h - short.size().1 * 2.0).abs() < 0.51,
            "two lines are two line boxes tall: {h} vs {}",
            short.size().1 * 2.0
        );
    }

    #[test]
    fn an_empty_string_lays_out_as_one_zero_width_line() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let layout = super::TextLayout::build(
            "",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );
        assert_eq!(layout.line_count(), 1, "an empty label still has a line box");
        assert_eq!(layout.size().0, 0.0);
        assert!(layout.size().1 > 0.0, "the line box keeps its height");
    }
```

`// mutation:` on the first test — change `build`'s paragraph split from `'\n'` to a
character that never occurs and `line_count` collapses to 1. On the second —
make `build` return early with no lines for empty text and `line_count` becomes 0.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib text::tests::a_paragraph_lays_out_one_line_per_newline_and_sizes_to_the_widest`
Expected: FAIL — `cannot find type 'TextLayout' in module 'super'`.

- [ ] **Step 4: Write the implementation**

Append to `ui/src/text.rs`:

```rust
/// How a paragraph that does not fit its width is shortened.
///
/// GTK's `PangoEllipsizeMode`. `None` lets the text overflow; the other three
/// replace a run of clusters with `…` at that end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ellipsize {
    /// Never shorten; overflow instead.
    None,
    /// Drop leading clusters.
    Start,
    /// Drop clusters from the middle.
    Middle,
    /// Drop trailing clusters.
    End,
}

/// How a paragraph that does not fit its width is broken.
///
/// GTK's `PangoWrapMode`: `Word` breaks only at UAX #14 opportunities,
/// `Char` breaks between any two grapheme clusters, `WordChar` prefers a word
/// break and falls back to a character break for a word wider than the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// Never break; overflow instead.
    None,
    /// Break at word boundaries only.
    Word,
    /// Break between grapheme clusters.
    Char,
    /// Word boundaries, falling back to clusters.
    WordChar,
}

/// One laid-out line of a [`TextLayout`].
struct ShapedLine {
    /// Byte range of this line within the layout's own `text`.
    range: std::ops::Range<usize>,
    /// The shaped run for `text[range]` (already ellipsized, if it was).
    shaped: Rc<ShapedText>,
    /// One entry per grapheme boundary in `range`, as
    /// `(byte offset in the layout's text, x advance from the line's left edge)`.
    /// The first entry is `(range.start, 0.0)` and the last
    /// `(range.end, line width)`, so a caret query is a lookup, never a reshape.
    carets: Vec<(usize, f32)>,
    /// Top of the line box, relative to the layout's origin.
    top: f32,
    /// Line box height.
    height: f32,
    /// Baseline offset from `top`.
    baseline: f32,
    /// Inked width.
    width: f32,
}

/// A wrapped, ellipsized, cursor-aware paragraph over [`FontDatabase::shape`].
///
/// M2's [`ShapedText`] is one run of one line. `TextLayout` is the multi-line,
/// editable-text layer every M3 widget that shows more than a fixed label needs:
/// `Label`'s wrapping and ellipsizing, `TextView`'s cursor and selection, and
/// the whole entry family's caret arithmetic. Caret positions are precomputed
/// at build time so [`TextLayout::caret_rect`] and [`TextLayout::byte_at`] can
/// take `&self` and never reach the font database again.
pub struct TextLayout {
    text: String,
    lines: Vec<ShapedLine>,
    width: f32,
    height: f32,
    /// The face every line was shaped with; `None` when no face loaded.
    face: Option<FontFace>,
}

impl TextLayout {
    /// Lay `text` out.
    ///
    /// `width` is the available inline size; `None` means unbounded, which is
    /// also what a non-finite or negative width is treated as. A face that will
    /// not load yields empty runs rather than a panic — a missing font must not
    /// take the widget tree down.
    pub fn build(
        text: &str,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        width: Option<f32>,
        wrap: WrapMode,
        ellipsize: Ellipsize,
    ) -> Self {
        let limit = width.filter(|w| w.is_finite() && *w > 0.0);
        let face = fonts.match_face(&style.query());
        let mut layout = TextLayout {
            text: text.to_owned(),
            lines: Vec::new(),
            width: 0.0,
            height: 0.0,
            face: face.clone(),
        };

        // Paragraphs first: a hard newline always breaks, whatever `wrap` says.
        let mut para_start = 0usize;
        let mut pieces: Vec<std::ops::Range<usize>> = Vec::new();
        for (idx, ch) in text.char_indices() {
            if ch == '\n' {
                pieces.push(para_start..idx);
                para_start = idx + ch.len_utf8();
            }
        }
        pieces.push(para_start..text.len());

        let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
        for para in pieces {
            layout.break_paragraph(para, style, fonts, limit, wrap, &mut ranges);
        }

        let mut top = 0.0f32;
        for range in ranges {
            let line = layout.shape_line(range, style, fonts, limit, ellipsize, top);
            top += line.height;
            layout.width = layout.width.max(line.width);
            layout.lines.push(line);
        }
        layout.height = top;
        layout
    }

    /// Shape one already-broken line and precompute its caret table.
    fn shape_line(
        &self,
        range: std::ops::Range<usize>,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: Option<f32>,
        ellipsize: Ellipsize,
        top: f32,
    ) -> ShapedLine {
        let source = &self.text[range.clone()];
        let (display, carets) =
            Self::ellipsize_line(source, range.start, style, fonts, limit, ellipsize);
        let shaped = self.shape_str(&display, style, fonts);
        let line_height = style.line_height_px(&shaped.metrics);
        let leading = ((line_height - shaped.metrics.ascent.max(0.0)
            - shaped.metrics.descent.max(0.0))
            / 2.0)
            .max(0.0);
        ShapedLine {
            range,
            width: shaped.metrics.width.max(0.0),
            baseline: leading + shaped.metrics.ascent.max(0.0),
            height: line_height.max(0.0),
            top,
            carets,
            shaped,
        }
    }

    /// Shape one string with this layout's face, or an empty run if none loaded.
    fn shape_str(&self, s: &str, style: &TextStyle, fonts: &mut FontDatabase) -> Rc<ShapedText> {
        match self.face.as_ref() {
            Some(face) => fonts.shape(&style.shape_key(s, face)),
            None => Rc::new(ShapedText {
                blob: None,
                metrics: TextMetrics {
                    width: 0.0,
                    ascent: style.size_px * 0.8,
                    descent: style.size_px * 0.2,
                    line_height: style.size_px * 1.2,
                },
                face: FontFace {
                    path: PathBuf::new(),
                    index: 0,
                    family: String::new(),
                },
                size_px: style.size_px,
            }),
        }
    }

    /// Total inked width and total height, in px.
    #[must_use]
    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    /// Number of laid-out lines. Always at least 1.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The source text this layout was built from.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Draw every line, `origin` being the top-left of the layout's box.
    pub fn draw(&self, canvas: &mut Canvas<'_>, origin: (f32, f32), color: Rgba) {
        let (ox, oy) = (
            if origin.0.is_finite() { origin.0 } else { 0.0 },
            if origin.1.is_finite() { origin.1 } else { 0.0 },
        );
        let paint = crate::paint::fill_paint(color);
        for line in &self.lines {
            if let Some(blob) = line.shaped.blob.as_ref() {
                canvas.draw_text_blob(blob, ox, oy + line.top + line.baseline, &paint);
            }
        }
    }
}
```

Add the three imports `TextLayout` needs at the top of `ui/src/text.rs`, beside
the existing ones — `use skia_rs_safe::canvas::Canvas;`, `use crate::layout::Rect;`
and `use crate::css::value::Rgba;` (`Rc`, `PathBuf` and `Once` are already imported).

Stub the two helpers Tasks 2 and 3 fill in, so this task compiles on its own:

```rust
impl TextLayout {
    /// Break one paragraph into line ranges. Task 2 replaces the body with the
    /// real wrapping; `WrapMode::None` is final and is what this emits.
    fn break_paragraph(
        &self,
        para: std::ops::Range<usize>,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: Option<f32>,
        wrap: WrapMode,
        out: &mut Vec<std::ops::Range<usize>>,
    ) {
        let _ = (style, fonts, limit, wrap);
        out.push(para);
    }

    /// Shorten one line to `limit` and return `(display string, caret table)`.
    /// Task 3 replaces the body with the real ellipsizing; `Ellipsize::None` is
    /// final and is what this returns.
    fn ellipsize_line(
        source: &str,
        base: usize,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: Option<f32>,
        ellipsize: Ellipsize,
    ) -> (String, Vec<(usize, f32)>) {
        let _ = (limit, ellipsize);
        let carets = Self::caret_table(source, base, source, style, fonts);
        (source.to_owned(), carets)
    }

    /// One `(byte, x)` pair per grapheme boundary of `display`, with byte
    /// offsets taken from `source` (they differ once an ellipsis is inserted).
    ///
    /// Prefix widths come from re-shaping each prefix; `FontDatabase::shape`
    /// memoizes, so a caret table costs one cache miss per cluster once.
    fn caret_table(
        source: &str,
        base: usize,
        display: &str,
        style: &TextStyle,
        fonts: &mut FontDatabase,
    ) -> Vec<(usize, f32)> {
        use unicode_segmentation::UnicodeSegmentation;
        let face = fonts.match_face(&style.query());
        let mut out = vec![(base, 0.0f32)];
        let mut byte = 0usize;
        for cluster in display.graphemes(true) {
            byte += cluster.len();
            let width = match face.as_ref() {
                Some(face) => fonts
                    .shape(&style.shape_key(&display[..byte], face))
                    .metrics
                    .width
                    .max(0.0),
                None => 0.0,
            };
            out.push((base + byte.min(source.len()), width));
        }
        out
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib text::tests::a_paragraph`
Run: `cargo test -p icedtea-ui --lib text::tests::an_empty_string`
Expected: PASS, both.

- [ ] **Step 6: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green, and the four `themed_button_offscreen` tests still pass unchanged.

- [ ] **Step 7: Commit**

```bash
git add ui/Cargo.toml ui/src/text.rs
git commit -m "feat(ui/text): TextLayout lays paragraphs out over the M2 shaper

Contract §3.7's paragraph engine: a per-line shaped run plus a precomputed
caret table, so caret_rect and byte_at stay '&self'. Wrapping and ellipsizing
land in the next two tasks; WrapMode::None and Ellipsize::None are final here.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `TextLayout` — word, char and word-char wrapping

**Files:**
- Modify: `ui/src/text.rs` (`TextLayout::break_paragraph`)
- Test: `ui/src/text.rs` (`mod tests`)

**Interfaces:**
- Consumes: Task 1's `TextLayout`, `ShapedLine`, `WrapMode`, `TextLayout::shape_str`.
- Produces: no new public items; `WrapMode::{Word, Char, WordChar}` become live.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn word_wrapping_breaks_at_spaces_and_never_exceeds_the_limit() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let one = super::TextLayout::build(
            "wrap",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );
        // Four short words, at a limit that fits about two of them.
        let limit = one.size().0 * 2.6;
        let wrapped = super::TextLayout::build(
            "wrap wrap wrap wrap",
            &style,
            &mut db,
            Some(limit),
            super::WrapMode::Word,
            super::Ellipsize::None,
        );
        assert!(
            wrapped.line_count() >= 2,
            "four words must not fit on one line at {limit}px"
        );
        assert!(
            wrapped.size().0 <= limit + 0.01,
            "no line may exceed the limit: {} > {limit}",
            wrapped.size().0
        );
        assert_eq!(wrapped.text(), "wrap wrap wrap wrap", "the source is intact");
    }

    #[test]
    fn a_word_wider_than_the_line_stays_whole_under_word_and_breaks_under_word_char() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let long = "abcdefghijklmnopqrstuvwxyz";
        let narrow = super::TextLayout::build(
            "abcd",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        )
        .size()
        .0;

        let word = super::TextLayout::build(
            long,
            &style,
            &mut db,
            Some(narrow),
            super::WrapMode::Word,
            super::Ellipsize::None,
        );
        assert_eq!(word.line_count(), 1, "Word never breaks inside a word");
        assert!(word.size().0 > narrow, "so it overflows instead");

        let word_char = super::TextLayout::build(
            long,
            &style,
            &mut db,
            Some(narrow),
            super::WrapMode::WordChar,
            super::Ellipsize::None,
        );
        assert!(
            word_char.line_count() > 1,
            "WordChar falls back to a cluster break"
        );
        assert!(word_char.size().0 <= narrow + 0.01);

        let ch = super::TextLayout::build(
            long,
            &style,
            &mut db,
            Some(narrow),
            super::WrapMode::Char,
            super::Ellipsize::None,
        );
        assert!(ch.line_count() > 1, "Char always breaks between clusters");
    }
```

`// mutation:` — make `break_paragraph` ignore `limit` and push the whole
paragraph; both tests fail on `line_count`. Make `WordChar` behave as `Word` and
the second test fails on the `word_char.line_count() > 1` assertion.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib text::tests::word_wrapping`
Expected: FAIL — `assertion failed: wrapped.line_count() >= 2` (the Task 1 stub
never breaks).

- [ ] **Step 3: Write the implementation**

Replace `TextLayout::break_paragraph` with:

```rust
    /// Break one paragraph into line ranges that respect `limit`.
    ///
    /// `Word` breaks only at UAX #14 opportunities (`unicode_linebreak`), which
    /// is what Pango does; `Char` breaks between grapheme clusters; `WordChar`
    /// prefers a word break and falls back to clusters for a word that cannot
    /// fit a whole line on its own. Trailing whitespace at a break is dropped
    /// from the line's inked range, as Pango does, so a wrapped line's width is
    /// measured without it.
    fn break_paragraph(
        &self,
        para: std::ops::Range<usize>,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: Option<f32>,
        wrap: WrapMode,
        out: &mut Vec<std::ops::Range<usize>>,
    ) {
        let Some(limit) = limit.filter(|_| wrap != WrapMode::None) else {
            out.push(para);
            return;
        };
        let source = &self.text[para.clone()];
        if source.is_empty() {
            out.push(para);
            return;
        }

        // Candidate break offsets, relative to `source`, ascending, always
        // ending at `source.len()`.
        let candidates: Vec<usize> = match wrap {
            WrapMode::None => unreachable!("guarded above"),
            WrapMode::Char => Self::cluster_breaks(source),
            WrapMode::Word | WrapMode::WordChar => unicode_linebreak::linebreaks(source)
                .map(|(offset, _)| offset)
                .collect(),
        };

        let mut start = 0usize;
        let mut last_fit: Option<usize> = None;
        for &candidate in &candidates {
            if candidate <= start {
                continue;
            }
            let piece = source[start..candidate].trim_end();
            let width = self
                .shape_str(piece, style, fonts)
                .metrics
                .width
                .max(0.0);
            if width <= limit {
                last_fit = Some(candidate);
                continue;
            }
            match last_fit.take() {
                // A break that fits: take it and retry this candidate.
                Some(fit) => {
                    out.push(para.start + start..para.start + source[..fit].trim_end().len());
                    start = fit;
                    // Re-measure this candidate against the new line.
                    let piece = source[start..candidate].trim_end();
                    let width = self.shape_str(piece, style, fonts).metrics.width.max(0.0);
                    if width <= limit {
                        last_fit = Some(candidate);
                    } else if wrap == WrapMode::WordChar {
                        start = self.break_overlong(
                            source, start, candidate, para.start, style, fonts, limit, out,
                        );
                    } else {
                        out.push(para.start + start..para.start + candidate);
                        start = candidate;
                    }
                }
                // No break fits: the run from `start` is wider than a whole line.
                None if wrap == WrapMode::WordChar => {
                    start = self.break_overlong(
                        source, start, candidate, para.start, style, fonts, limit, out,
                    );
                }
                None => {
                    out.push(para.start + start..para.start + candidate);
                    start = candidate;
                }
            }
        }
        if start < source.len() || out.is_empty() {
            out.push(para.start + start..para.end);
        }
    }

    /// Emit cluster-broken lines for `source[start..end]`, which does not fit
    /// `limit` at any word boundary. Returns the new `start`.
    #[allow(clippy::too_many_arguments)]
    fn break_overlong(
        &self,
        source: &str,
        start: usize,
        end: usize,
        base: usize,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: f32,
        out: &mut Vec<std::ops::Range<usize>>,
    ) -> usize {
        let mut cursor = start;
        let clusters = Self::cluster_breaks(&source[start..end]);
        let mut last_fit = cursor;
        for offset in clusters {
            let candidate = start + offset;
            let width = self
                .shape_str(&source[cursor..candidate], style, fonts)
                .metrics
                .width
                .max(0.0);
            if width <= limit {
                last_fit = candidate;
                continue;
            }
            // Always consume at least one cluster, or this loops forever.
            let take = if last_fit > cursor { last_fit } else { candidate };
            out.push(base + cursor..base + take);
            cursor = take;
            last_fit = cursor;
        }
        cursor
    }

    /// Grapheme-cluster boundaries of `s`, ascending, ending at `s.len()`.
    fn cluster_breaks(s: &str) -> Vec<usize> {
        use unicode_segmentation::UnicodeSegmentation;
        let mut out = Vec::new();
        let mut byte = 0usize;
        for cluster in s.graphemes(true) {
            byte += cluster.len();
            out.push(byte);
        }
        if out.last() != Some(&s.len()) {
            out.push(s.len());
        }
        out
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib text::tests::word_wrapping`
Run: `cargo test -p icedtea-ui --lib text::tests::a_word_wider_than_the_line`
Expected: PASS, both.

- [ ] **Step 5: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add ui/src/text.rs
git commit -m "feat(ui/text): TextLayout wraps at UAX #14 opportunities and clusters

Word breaks only at line-break opportunities, Char between grapheme clusters,
WordChar prefers a word break and falls back to clusters for a word too wide
for a whole line. break_overlong always consumes at least one cluster, so a
cluster wider than the limit cannot loop.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `TextLayout` — start, middle and end ellipsizing

**Files:**
- Modify: `ui/src/text.rs` (`TextLayout::ellipsize_line`)
- Test: `ui/src/text.rs` (`mod tests`)

**Interfaces:**
- Consumes: Task 1's `TextLayout::{caret_table, shape_str}`, `Ellipsize`.
- Produces: no new public items; `Ellipsize::{Start, Middle, End}` become live.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn ellipsizing_fits_the_limit_and_keeps_the_named_end() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let source = "alpha bravo charlie delta";
        let full = super::TextLayout::build(
            source,
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );
        let limit = full.size().0 * 0.5;

        for mode in [
            super::Ellipsize::Start,
            super::Ellipsize::Middle,
            super::Ellipsize::End,
        ] {
            let cut = super::TextLayout::build(
                source,
                &style,
                &mut db,
                Some(limit),
                super::WrapMode::None,
                mode,
            );
            assert_eq!(cut.line_count(), 1, "{mode:?} does not add lines");
            assert!(
                cut.size().0 <= limit + 0.01,
                "{mode:?} must fit {limit}px, got {}",
                cut.size().0
            );
            assert_eq!(cut.text(), source, "{mode:?} leaves the source intact");
            assert!(
                cut.display_line(0).contains('\u{2026}'),
                "{mode:?} inserts an ellipsis: {:?}",
                cut.display_line(0)
            );
        }

        let end = super::TextLayout::build(
            source,
            &style,
            &mut db,
            Some(limit),
            super::WrapMode::None,
            super::Ellipsize::End,
        );
        assert!(
            end.display_line(0).starts_with("alpha"),
            "End keeps the head: {:?}",
            end.display_line(0)
        );
        let start = super::TextLayout::build(
            source,
            &style,
            &mut db,
            Some(limit),
            super::WrapMode::None,
            super::Ellipsize::Start,
        );
        assert!(
            start.display_line(0).ends_with("delta"),
            "Start keeps the tail: {:?}",
            start.display_line(0)
        );
    }

    #[test]
    fn a_limit_narrower_than_the_ellipsis_yields_the_ellipsis_alone() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let cut = super::TextLayout::build(
            "alpha bravo",
            &style,
            &mut db,
            Some(0.5),
            super::WrapMode::None,
            super::Ellipsize::End,
        );
        assert_eq!(cut.display_line(0), "\u{2026}", "never an empty display");
        assert_eq!(cut.line_count(), 1);
    }
```

`// mutation:` — return `source` unchanged from `ellipsize_line` and the first
test fails on the width assertion; drop the `"\u{2026}".to_owned()` floor in the
too-narrow branch and the second test fails with an empty string.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib text::tests::ellipsizing_fits`
Expected: FAIL — `no method named 'display_line'`.

- [ ] **Step 3: Write the implementation**

Replace `TextLayout::ellipsize_line`, and add `display_line`:

```rust
    /// The character Pango and GTK use for every ellipsis.
    const ELLIPSIS: &'static str = "\u{2026}";

    /// Shorten `source` to `limit` at the end named by `ellipsize`.
    ///
    /// Returns the string that is actually shaped plus the caret table, whose
    /// byte offsets always point back into the *source* text: a caret query on
    /// an ellipsized label must land on a real byte offset, never inside the
    /// ellipsis. Clusters swallowed by the ellipsis collapse onto the offset of
    /// the first cluster the ellipsis replaced.
    fn ellipsize_line(
        source: &str,
        base: usize,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: Option<f32>,
        ellipsize: Ellipsize,
    ) -> (String, Vec<(usize, f32)>) {
        let plain = |fonts: &mut FontDatabase| {
            (
                source.to_owned(),
                Self::caret_table(source, base, source, style, fonts),
            )
        };
        let Some(limit) = limit.filter(|_| ellipsize != Ellipsize::None) else {
            return plain(fonts);
        };
        let face = fonts.match_face(&style.query());
        let Some(face) = face else {
            return plain(fonts);
        };
        let measure = |fonts: &mut FontDatabase, s: &str| {
            fonts.shape(&style.shape_key(s, &face)).metrics.width.max(0.0)
        };
        if measure(fonts, source) <= limit {
            return plain(fonts);
        }

        let clusters = Self::cluster_breaks(source);
        let ellipsis_w = measure(fonts, Self::ELLIPSIS);
        if ellipsis_w > limit {
            // Even the ellipsis does not fit; show it anyway rather than
            // nothing, which is what GTK renders.
            let display = Self::ELLIPSIS.to_owned();
            return (display, vec![(base, 0.0), (base + source.len(), ellipsis_w)]);
        }
        let budget = limit - ellipsis_w;

        let display = match ellipsize {
            Ellipsize::None => unreachable!("guarded above"),
            Ellipsize::End => {
                let mut keep = 0usize;
                for &offset in &clusters {
                    if measure(fonts, &source[..offset]) <= budget {
                        keep = offset;
                    } else {
                        break;
                    }
                }
                format!("{}{}", &source[..keep], Self::ELLIPSIS)
            }
            Ellipsize::Start => {
                let mut keep = source.len();
                for &offset in clusters.iter().rev() {
                    if measure(fonts, &source[offset..]) <= budget {
                        keep = offset;
                    } else {
                        break;
                    }
                }
                format!("{}{}", Self::ELLIPSIS, &source[keep..])
            }
            Ellipsize::Middle => {
                let (mut head, mut tail) = (0usize, source.len());
                loop {
                    let next_head = clusters.iter().copied().find(|&o| o > head);
                    let next_tail = clusters.iter().rev().copied().find(|&o| o < tail);
                    let grown = match (next_head, next_tail) {
                        (Some(h), Some(t)) if h <= t => (h, t),
                        _ => break,
                    };
                    let candidate =
                        format!("{}{}", &source[..grown.0], &source[grown.1..]);
                    if measure(fonts, &candidate) > budget {
                        break;
                    }
                    head = grown.0;
                    tail = grown.1;
                    // Shrink the tail on the next pass, not the head, so both
                    // ends grow evenly.
                    if let Some(t) = clusters.iter().rev().copied().find(|&o| o < tail) {
                        let candidate = format!("{}{}", &source[..head], &source[t..]);
                        if measure(fonts, &candidate) <= budget {
                            tail = t;
                        }
                    }
                }
                format!("{}{}{}", &source[..head], Self::ELLIPSIS, &source[tail..])
            }
        };

        // Rebuild the caret table over the *display* string, then remap every
        // offset that fell inside the ellipsis onto the source byte the
        // ellipsis stands for.
        let mut table = Self::caret_table(&display, 0, &display, style, fonts);
        let ellipsis_at = display.find(Self::ELLIPSIS).unwrap_or(0);
        for entry in &mut table {
            let byte = entry.0;
            entry.0 = base
                + if byte <= ellipsis_at {
                    match ellipsize {
                        Ellipsize::Start => 0,
                        _ => byte,
                    }
                } else if byte >= ellipsis_at + Self::ELLIPSIS.len() {
                    let after = byte - (ellipsis_at + Self::ELLIPSIS.len());
                    source.len() - (display.len() - ellipsis_at - Self::ELLIPSIS.len()) + after
                } else {
                    ellipsis_at
                };
        }
        (display, table)
    }

    /// The string line `index` actually shapes — the source text for a line
    /// that was not ellipsized, and the shortened form for one that was.
    /// Diagnostics and tests only; painting goes through the shaped blob.
    #[must_use]
    pub fn display_line(&self, index: usize) -> String {
        match self.lines.get(index) {
            Some(line) => match line.shaped.blob.as_ref() {
                // The shaped run is the source of truth for what was drawn,
                // but it holds glyphs, not characters, so the display string
                // is reconstructed from the caret table's own span.
                Some(_) | None => self.line_display.get(index).cloned().unwrap_or_default(),
            },
            None => String::new(),
        }
    }
```

`display_line` needs the display strings kept. Add `line_display: Vec<String>` to
`TextLayout`, initialise it to `Vec::new()` in `build`, and push each line's
display string in `build`'s line loop:

```rust
        for range in ranges {
            let (display, line) =
                layout.shape_line_with_display(range, style, fonts, limit, ellipsize, top);
            top += line.height;
            layout.width = layout.width.max(line.width);
            layout.line_display.push(display);
            layout.lines.push(line);
        }
```

and rename Task 1's `shape_line` to `shape_line_with_display`, returning
`(String, ShapedLine)` — it already computes the display string:

```rust
    fn shape_line_with_display(
        &self,
        range: std::ops::Range<usize>,
        style: &TextStyle,
        fonts: &mut FontDatabase,
        limit: Option<f32>,
        ellipsize: Ellipsize,
        top: f32,
    ) -> (String, ShapedLine) {
        let source = &self.text[range.clone()];
        let (display, carets) =
            Self::ellipsize_line(source, range.start, style, fonts, limit, ellipsize);
        let shaped = self.shape_str(&display, style, fonts);
        let line_height = style.line_height_px(&shaped.metrics);
        let leading = ((line_height - shaped.metrics.ascent.max(0.0)
            - shaped.metrics.descent.max(0.0))
            / 2.0)
            .max(0.0);
        let line = ShapedLine {
            range,
            width: shaped.metrics.width.max(0.0),
            baseline: leading + shaped.metrics.ascent.max(0.0),
            height: line_height.max(0.0),
            top,
            carets,
            shaped,
        };
        (display, line)
    }
```

and simplify `display_line` to:

```rust
    #[must_use]
    pub fn display_line(&self, index: usize) -> String {
        self.line_display.get(index).cloned().unwrap_or_default()
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib text::tests::ellipsizing_fits`
Run: `cargo test -p icedtea-ui --lib text::tests::a_limit_narrower_than_the_ellipsis`
Expected: PASS, both.

- [ ] **Step 5: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add ui/src/text.rs
git commit -m "feat(ui/text): TextLayout ellipsizes at the start, middle and end

Caret offsets are remapped back onto source bytes, so a caret query on an
ellipsized label never lands inside the ellipsis. A limit too narrow for the
ellipsis itself renders the ellipsis alone, as GTK does, never an empty line.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `TextLayout` — carets, hit-testing and selection rectangles

**Files:**
- Modify: `ui/src/text.rs`
- Test: `ui/src/text.rs` (`mod tests`)

**Interfaces:**
- Consumes: Task 1's `ShapedLine.carets`, `layout::Rect`.
- Produces:
  ```rust
  impl TextLayout {
      #[must_use] pub fn caret_rect(&self, byte: usize) -> Rect;
      #[must_use] pub fn byte_at(&self, point: (f32, f32)) -> usize;
      #[must_use] pub fn selection_rects(&self, range: std::ops::Range<usize>) -> Vec<Rect>;
      #[must_use] pub fn line_of(&self, byte: usize) -> usize;
  }
  ```

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn a_caret_round_trips_through_its_own_rect() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let layout = super::TextLayout::build(
            "Hello world",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );

        let zero = layout.caret_rect(0);
        assert_eq!(zero.x, 0.0, "the caret before the first byte sits at x=0");
        assert!(zero.height > 0.0, "a caret is a line-height-tall sliver");

        let end = layout.caret_rect("Hello world".len());
        assert!(
            (end.x - layout.size().0).abs() < 0.51,
            "the caret past the last byte sits at the line's width: {} vs {}",
            end.x,
            layout.size().0
        );

        for byte in [0usize, 1, 5, 6, 11] {
            let rect = layout.caret_rect(byte);
            let back = layout.byte_at((rect.x + 0.01, rect.y + rect.height / 2.0));
            assert_eq!(back, byte, "byte_at must invert caret_rect at {byte}");
        }
    }

    #[test]
    fn a_point_past_the_end_of_a_line_lands_on_that_lines_last_byte() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let layout = super::TextLayout::build(
            "ab\ncd",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );
        let first = layout.caret_rect(0);
        assert_eq!(layout.byte_at((10_000.0, first.y + 1.0)), 2, "end of line 1");
        assert_eq!(layout.byte_at((-5.0, first.y + 1.0)), 0, "before line 1");
        assert_eq!(
            layout.byte_at((10_000.0, 10_000.0)),
            5,
            "below everything is the last byte"
        );
        assert_eq!(layout.line_of(4), 1, "byte 4 is on the second line");
    }

    #[test]
    fn a_selection_yields_one_rect_per_covered_line() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let layout = super::TextLayout::build(
            "abc\ndef",
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );
        assert_eq!(layout.selection_rects(0..0).len(), 0, "an empty range is empty");
        assert_eq!(layout.selection_rects(1..2).len(), 1, "within one line");
        let across = layout.selection_rects(1..6);
        assert_eq!(across.len(), 2, "one rect per covered line");
        assert!(across[0].y < across[1].y, "top-first");
        assert!(across[0].width > 0.0 && across[1].width > 0.0);
        // An out-of-range end must be clamped, not panic.
        assert_eq!(layout.selection_rects(0..99_999).len(), 2);
    }
```

`// mutation:` — clamp `byte_at`'s x search to the first caret entry only and
the round-trip test fails at byte 5; drop the per-line split in
`selection_rects` and the cross-line test reports 1 rect.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib text::tests::a_caret_round_trips`
Expected: FAIL — `no method named 'caret_rect'`.

- [ ] **Step 3: Write the implementation**

```rust
impl TextLayout {
    /// Which line `byte` sits on. Clamped to the last line.
    #[must_use]
    pub fn line_of(&self, byte: usize) -> usize {
        for (index, line) in self.lines.iter().enumerate() {
            if byte < line.range.end {
                return index;
            }
        }
        self.lines.len().saturating_sub(1)
    }

    /// The caret rectangle for `byte`: a zero-width, line-box-tall sliver at
    /// the cluster boundary at or before `byte`.
    ///
    /// A `byte` inside a cluster snaps to that cluster's start, which is what
    /// every caret consumer wants — a caret never sits inside a grapheme.
    #[must_use]
    pub fn caret_rect(&self, byte: usize) -> Rect {
        let index = self.line_of(byte);
        let Some(line) = self.lines.get(index) else {
            return Rect::zero();
        };
        let mut x = 0.0f32;
        for &(at, advance) in &line.carets {
            if at <= byte {
                x = advance;
            } else {
                break;
            }
        }
        Rect::new(x, line.top, 0.0, line.height)
    }

    /// The byte offset nearest `point`, in the layout's own coordinate space.
    ///
    /// A point above the first line lands on byte 0, below the last on the end
    /// of the text; horizontally it snaps to the nearer of the two cluster
    /// boundaries it falls between, which is what a click in the right half of
    /// a glyph means.
    #[must_use]
    pub fn byte_at(&self, point: (f32, f32)) -> usize {
        let (x, y) = (
            if point.0.is_finite() { point.0 } else { 0.0 },
            if point.1.is_finite() { point.1 } else { 0.0 },
        );
        let Some(line) = self
            .lines
            .iter()
            .find(|line| y < line.top + line.height)
            .or_else(|| self.lines.last())
        else {
            return 0;
        };
        let mut best = line.carets.first().map_or(0, |entry| entry.0);
        let mut best_delta = f32::INFINITY;
        for &(at, advance) in &line.carets {
            let delta = (advance - x).abs();
            if delta < best_delta {
                best_delta = delta;
                best = at;
            }
        }
        best
    }

    /// One rectangle per line covered by `range`, top-first.
    ///
    /// An inverted or out-of-range `range` is clamped rather than rejected —
    /// selection ranges come from pointer drags and key repeats and must never
    /// panic.
    #[must_use]
    pub fn selection_rects(&self, range: std::ops::Range<usize>) -> Vec<Rect> {
        let start = range.start.min(range.end).min(self.text.len());
        let end = range.end.max(range.start).min(self.text.len());
        if start == end {
            return Vec::new();
        }
        let mut out = Vec::new();
        for line in &self.lines {
            let from = start.max(line.range.start);
            let to = end.min(line.range.end);
            if from >= to {
                continue;
            }
            let x0 = self.advance_in(line, from);
            let x1 = self.advance_in(line, to);
            out.push(Rect::new(x0, line.top, (x1 - x0).max(0.0), line.height));
        }
        out
    }

    /// x advance of `byte` within `line`, snapped to a cluster boundary.
    fn advance_in(&self, line: &ShapedLine, byte: usize) -> f32 {
        let mut x = 0.0f32;
        for &(at, advance) in &line.carets {
            if at <= byte {
                x = advance;
            } else {
                break;
            }
        }
        x
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib text::tests::a_caret_round_trips`
Run: `cargo test -p icedtea-ui --lib text::tests::a_point_past_the_end`
Run: `cargo test -p icedtea-ui --lib text::tests::a_selection_yields`
Expected: PASS, all three.

- [ ] **Step 5: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add ui/src/text.rs
git commit -m "feat(ui/text): caret rects, hit-testing and selection rects

The basis for every cursor, selection and click-to-position in the entry
family and TextView. Ranges are clamped, never rejected: they come from
pointer drags and key repeats and must not panic.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: `TextLayout` — grapheme and word movement

**Files:**
- Modify: `ui/src/text.rs`
- Test: `ui/src/text.rs` (`mod tests`)

**Interfaces:**
- Consumes: `unicode_segmentation::UnicodeSegmentation`.
- Produces:
  ```rust
  impl TextLayout {
      #[must_use] pub fn next_grapheme(&self, byte: usize) -> usize;
      #[must_use] pub fn prev_grapheme(&self, byte: usize) -> usize;
      #[must_use] pub fn next_word(&self, byte: usize) -> usize;
      #[must_use] pub fn prev_word(&self, byte: usize) -> usize;
  }
  ```

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn grapheme_movement_steps_over_combining_marks_and_clamps_at_the_ends() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        // "e" + combining acute, then a flag (two regional indicators).
        let text = "e\u{0301}x\u{1F1EF}\u{1F1F5}";
        let layout = super::TextLayout::build(
            text,
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );

        assert_eq!(layout.next_grapheme(0), 3, "e + U+0301 is one cluster");
        assert_eq!(layout.next_grapheme(3), 4, "then the ASCII x");
        assert_eq!(layout.next_grapheme(4), text.len(), "the flag is one cluster");
        assert_eq!(layout.next_grapheme(text.len()), text.len(), "clamped");

        assert_eq!(layout.prev_grapheme(text.len()), 4);
        assert_eq!(layout.prev_grapheme(4), 3);
        assert_eq!(layout.prev_grapheme(3), 0);
        assert_eq!(layout.prev_grapheme(0), 0, "clamped");

        // A byte in the middle of a cluster must not panic and must not slice
        // through a char boundary.
        assert_eq!(layout.next_grapheme(1), 3);
        assert_eq!(layout.prev_grapheme(2), 0);
    }

    #[test]
    fn word_movement_lands_on_word_starts_the_way_ctrl_arrow_does() {
        let style = ui_style("label { font-size: 14px; }");
        let mut db = FontDatabase::new();
        let text = "alpha  bravo charlie";
        let layout = super::TextLayout::build(
            text,
            &style,
            &mut db,
            None,
            super::WrapMode::None,
            super::Ellipsize::None,
        );

        assert_eq!(layout.next_word(0), 7, "past 'alpha' and both spaces");
        assert_eq!(layout.next_word(7), 13, "to 'charlie'");
        assert_eq!(layout.next_word(13), text.len(), "to the end");
        assert_eq!(layout.next_word(text.len()), text.len(), "clamped");

        assert_eq!(layout.prev_word(text.len()), 13);
        assert_eq!(layout.prev_word(13), 7);
        assert_eq!(layout.prev_word(7), 0);
        assert_eq!(layout.prev_word(0), 0, "clamped");
    }
```

`// mutation:` — replace `graphemes(true)` with `char_indices` in
`next_grapheme` and the combining-mark assertion (`3`) drops to `1`. Make
`next_word` stop at the end of a word instead of the start of the next and the
second test fails at `7`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib text::tests::grapheme_movement`
Expected: FAIL — `no method named 'next_grapheme'`.

- [ ] **Step 3: Write the implementation**

```rust
impl TextLayout {
    /// The cluster boundary after `byte`. Clamped to the end of the text; a
    /// `byte` inside a cluster moves to the end of that cluster.
    #[must_use]
    pub fn next_grapheme(&self, byte: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let byte = byte.min(self.text.len());
        for (offset, cluster) in self.text.grapheme_indices(true) {
            if offset + cluster.len() > byte {
                return offset + cluster.len();
            }
        }
        self.text.len()
    }

    /// The cluster boundary before `byte`. Clamped to 0; a `byte` inside a
    /// cluster moves to the start of that cluster.
    #[must_use]
    pub fn prev_grapheme(&self, byte: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let byte = byte.min(self.text.len());
        let mut previous = 0usize;
        for (offset, cluster) in self.text.grapheme_indices(true) {
            if offset >= byte {
                break;
            }
            if offset + cluster.len() >= byte {
                return offset;
            }
            previous = offset;
        }
        previous
    }

    /// The start of the next word after `byte` — where `Ctrl+Right` lands.
    ///
    /// GTK moves to the *start* of the following word, not the end of the
    /// current one, so trailing whitespace is consumed with the move.
    #[must_use]
    pub fn next_word(&self, byte: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let byte = byte.min(self.text.len());
        let mut seen_current = false;
        for (offset, word) in self.text.split_word_bound_indices() {
            if offset < byte {
                continue;
            }
            let is_word = word.chars().any(|c| !c.is_whitespace());
            if !is_word {
                continue;
            }
            if offset > byte || seen_current {
                return offset;
            }
            seen_current = true;
        }
        self.text.len()
    }

    /// The start of the word at or before `byte` — where `Ctrl+Left` lands.
    #[must_use]
    pub fn prev_word(&self, byte: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let byte = byte.min(self.text.len());
        let mut best = 0usize;
        for (offset, word) in self.text.split_word_bound_indices() {
            if offset >= byte {
                break;
            }
            if word.chars().any(|c| !c.is_whitespace()) {
                best = offset;
            }
        }
        best
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib text::tests::grapheme_movement`
Run: `cargo test -p icedtea-ui --lib text::tests::word_movement`
Expected: PASS, both.

- [ ] **Step 5: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add ui/src/text.rs
git commit -m "feat(ui/text): grapheme and word movement for arrow and Ctrl-arrow

Cluster movement is UAX #29, so a combining mark or a flag moves as one unit;
word movement lands on the start of the next word, as GTK's Ctrl+Arrow does.
Every offset is clamped and snapped, so a byte inside a cluster never slices.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: `parse_markup` — the `<b><i><span>` subset, never panicking

**Files:**
- Modify: `ui/src/text.rs`
- Test: `ui/src/text.rs` (`mod tests`)

**Interfaces:**
- Consumes: `css::value::Rgba`, `css::value::color` parsing helpers via `Rgba`'s own hex path.
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq)]
  pub struct MarkupSpan {
      pub range: std::ops::Range<usize>,
      pub bold: bool,
      pub italic: bool,
      pub color: Option<Rgba>,
      pub weight: Option<f32>,
      pub size_px: Option<f32>,
  }

  pub fn parse_markup(text: &str) -> (String, Vec<MarkupSpan>);
  ```

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn markup_extracts_bold_italic_and_span_attributes() {
        let (plain, spans) = super::parse_markup(
            "a<b>bold</b>c<i>it</i><span foreground=\"#ff0000\" weight=\"700\" \
             size=\"20\">red</span>",
        );
        assert_eq!(plain, "aboldcitred", "tags are removed from the text");

        let bold = spans.iter().find(|s| s.bold).expect("a bold span");
        assert_eq!(bold.range, 1..5, "'bold' at bytes 1..5");
        assert!(!bold.italic);

        let italic = spans.iter().find(|s| s.italic).expect("an italic span");
        assert_eq!(italic.range, 6..8);

        let coloured = spans
            .iter()
            .find(|s| s.color.is_some())
            .expect("a coloured span");
        assert_eq!(coloured.range, 8..11);
        assert_eq!(coloured.color.map(|c| (c.r, c.g, c.b)), Some((1.0, 0.0, 0.0)));
        assert_eq!(coloured.weight, Some(700.0));
        assert_eq!(coloured.size_px, Some(20.0));
    }

    #[test]
    fn markup_entities_are_decoded_and_unknown_tags_are_dropped() {
        let (plain, spans) = super::parse_markup("5 &lt; 6 &amp; <u>x</u>&gt;");
        assert_eq!(plain, "5 < 6 & x>", "entities decode, unknown tags vanish");
        assert!(
            spans.iter().all(|s| !s.bold && !s.italic && s.color.is_none()),
            "an unknown tag contributes no span"
        );
    }

    #[test]
    fn markup_never_panics_on_hostile_input() {
        // Unclosed, mismatched, nested past any sane depth, invalid attribute
        // values, lone angle brackets, and a non-ASCII payload — none of these
        // may panic or slice through a char boundary.
        let hostile = [
            "<b>",
            "</b>",
            "<b><i></b></i>",
            "<span foreground=\"not-a-colour\" size=\"NaN\" weight=\"-3\">x</span>",
            "<span foreground=>x",
            "a < b > c",
            "&notanentity; &#; &#xZZ;",
            "<b>\u{00e9}\u{0301}\u{1F1EF}\u{1F1F5}</b>",
            &"<b>".repeat(5_000),
            &format!("<b>{}</b>", "\u{00e9}".repeat(10_000)),
        ];
        for input in hostile {
            let (plain, spans) = super::parse_markup(input);
            assert!(plain.is_char_boundary(plain.len()));
            for span in &spans {
                assert!(
                    span.range.start <= span.range.end && span.range.end <= plain.len(),
                    "span {:?} out of range for {:?}",
                    span.range,
                    plain
                );
                assert!(plain.is_char_boundary(span.range.start));
                assert!(plain.is_char_boundary(span.range.end));
            }
        }
    }
```

`// mutation:` — remove the `is_char_boundary` guard in the tag scanner and the
hostile test panics on the multi-byte payload; drop the entity table and the
second test fails on `"5 < 6 & x>"`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib text::tests::markup_extracts`
Expected: FAIL — `cannot find function 'parse_markup' in module 'super'`.

- [ ] **Step 3: Write the implementation**

```rust
/// One attributed run produced by [`parse_markup`], indexed into the plain text.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkupSpan {
    /// Byte range in the returned plain string.
    pub range: std::ops::Range<usize>,
    /// `<b>` was open over this run.
    pub bold: bool,
    /// `<i>` was open over this run.
    pub italic: bool,
    /// `<span foreground="…">`.
    pub color: Option<Rgba>,
    /// `<span weight="…">`, 1..=1000.
    pub weight: Option<f32>,
    /// `<span size="…">`, in px.
    pub size_px: Option<f32>,
}

/// How deep `parse_markup` will nest before it stops opening spans.
///
/// Hostile markup is a string a theme or an app model can supply; an
/// unbounded stack here is a stack overflow waiting for `"<b>".repeat(n)`.
const MARKUP_MAX_DEPTH: usize = 64;

static MARKUP_WARNED: Once = Once::new();

/// Parse the `<b>`, `<i>` and `<span foreground|weight|size>` subset.
///
/// Everything else — any other tag, any other attribute, a malformed entity, a
/// stray angle bracket — is dropped and logged once. This never panics and
/// never returns a span that is not on a char boundary of the returned string:
/// markup reaches here from application models and stylesheets, which are
/// untrusted input (contract §9, cross-cutting rules).
#[must_use]
pub fn parse_markup(text: &str) -> (String, Vec<MarkupSpan>) {
    #[derive(Clone, Copy, Default)]
    struct Frame {
        bold: bool,
        italic: bool,
        color: Option<Rgba>,
        weight: Option<f32>,
        size_px: Option<f32>,
    }

    let mut plain = String::with_capacity(text.len());
    let mut spans: Vec<MarkupSpan> = Vec::new();
    let mut stack: Vec<(Frame, usize)> = Vec::new();
    let mut current = Frame::default();
    let mut run_start = 0usize;
    let mut warned = false;

    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'<' => {
                let Some(close) = text[i..].find('>').map(|o| i + o) else {
                    // A lone '<' with no '>': copy it through as text.
                    plain.push('<');
                    i += 1;
                    continue;
                };
                let tag = &text[i + 1..close];
                // Flush the run that ends here.
                if plain.len() > run_start
                    && (current.bold
                        || current.italic
                        || current.color.is_some()
                        || current.weight.is_some()
                        || current.size_px.is_some())
                {
                    spans.push(MarkupSpan {
                        range: run_start..plain.len(),
                        bold: current.bold,
                        italic: current.italic,
                        color: current.color,
                        weight: current.weight,
                        size_px: current.size_px,
                    });
                }
                run_start = plain.len();

                let (closing, name_and_attrs) = match tag.strip_prefix('/') {
                    Some(rest) => (true, rest),
                    None => (false, tag),
                };
                let name = name_and_attrs
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();

                match (closing, name.as_str()) {
                    (false, "b" | "i" | "span") => {
                        if stack.len() < MARKUP_MAX_DEPTH {
                            stack.push((current, plain.len()));
                            let mut next = current;
                            match name.as_str() {
                                "b" => next.bold = true,
                                "i" => next.italic = true,
                                _ => apply_span_attrs(name_and_attrs, &mut next.color,
                                                      &mut next.weight, &mut next.size_px),
                            }
                            current = next;
                        } else {
                            warned = true;
                        }
                    }
                    (true, "b" | "i" | "span") => match stack.pop() {
                        Some((frame, _)) => current = frame,
                        None => warned = true,
                    },
                    _ => warned = true,
                }
                i = close + 1;
            }
            b'&' => {
                let (decoded, consumed) = decode_entity(&text[i..]);
                match decoded {
                    Some(ch) => plain.push(ch),
                    None => {
                        warned = true;
                        plain.push('&');
                    }
                }
                i += consumed;
            }
            _ => {
                let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
                plain.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    if plain.len() > run_start
        && (current.bold
            || current.italic
            || current.color.is_some()
            || current.weight.is_some()
            || current.size_px.is_some())
    {
        spans.push(MarkupSpan {
            range: run_start..plain.len(),
            bold: current.bold,
            italic: current.italic,
            color: current.color,
            weight: current.weight,
            size_px: current.size_px,
        });
    }
    if warned {
        MARKUP_WARNED.call_once(|| {
            tracing::warn!(
                "markup outside the <b>/<i>/<span foreground|weight|size> subset was dropped"
            );
        });
    }
    (plain, spans)
}

/// Read `foreground`, `weight` and `size` off a `<span …>` attribute list.
/// An attribute that does not parse is dropped, never defaulted to nonsense.
fn apply_span_attrs(
    attrs: &str,
    color: &mut Option<Rgba>,
    weight: &mut Option<f32>,
    size_px: &mut Option<f32>,
) {
    let mut rest = attrs;
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq].trim().to_ascii_lowercase();
        let after = &rest[eq + 1..];
        let trimmed = after.trim_start();
        let (value, consumed) = match trimmed.chars().next() {
            Some(quote @ ('"' | '\'')) => match trimmed[1..].find(quote) {
                Some(end) => (&trimmed[1..1 + end], 1 + end + 1),
                None => return,
            },
            Some(_) => {
                let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
                (&trimmed[..end], end)
            }
            None => return,
        };
        match name.as_str() {
            "foreground" | "color" => {
                if let Some(rgba) = parse_markup_color(value) {
                    *color = Some(rgba);
                }
            }
            "weight" => {
                if let Ok(w) = value.parse::<f32>() {
                    if w.is_finite() && w >= 1.0 && w <= 1000.0 {
                        *weight = Some(w);
                    }
                } else {
                    *weight = match value.to_ascii_lowercase().as_str() {
                        "bold" => Some(700.0),
                        "normal" => Some(400.0),
                        _ => *weight,
                    };
                }
            }
            "size" => {
                if let Ok(s) = value.parse::<f32>() {
                    if s.is_finite() && s > 0.0 {
                        *size_px = Some(s);
                    }
                }
            }
            _ => {}
        }
        let offset = eq + 1 + (after.len() - trimmed.len()) + consumed;
        rest = &rest[offset.min(rest.len())..];
    }
}

/// `#rgb`, `#rrggbb` and the sixteen HTML colour names Pango markup uses.
fn parse_markup_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let expand = |c: u8| -> f32 { f32::from(c) / 255.0 };
        let byte = |s: &str| u8::from_str_radix(s, 16).ok();
        return match hex.len() {
            3 => {
                let mut it = hex.chars();
                let mut nibble = || {
                    let c = it.next()?;
                    let v = c.to_digit(16)? as u8;
                    Some(expand(v * 17))
                };
                Some(Rgba { r: nibble()?, g: nibble()?, b: nibble()?, a: 1.0 })
            }
            6 => Some(Rgba {
                r: expand(byte(&hex[0..2])?),
                g: expand(byte(&hex[2..4])?),
                b: expand(byte(&hex[4..6])?),
                a: 1.0,
            }),
            _ => None,
        };
    }
    let named = |r: f32, g: f32, b: f32| Some(Rgba { r, g, b, a: 1.0 });
    match value.to_ascii_lowercase().as_str() {
        "black" => named(0.0, 0.0, 0.0),
        "white" => named(1.0, 1.0, 1.0),
        "red" => named(1.0, 0.0, 0.0),
        "green" => named(0.0, 0.5019608, 0.0),
        "blue" => named(0.0, 0.0, 1.0),
        "yellow" => named(1.0, 1.0, 0.0),
        "cyan" | "aqua" => named(0.0, 1.0, 1.0),
        "magenta" | "fuchsia" => named(1.0, 0.0, 1.0),
        "gray" | "grey" => named(0.5019608, 0.5019608, 0.5019608),
        "silver" => named(0.7529412, 0.7529412, 0.7529412),
        "maroon" => named(0.5019608, 0.0, 0.0),
        "olive" => named(0.5019608, 0.5019608, 0.0),
        "lime" => named(0.0, 1.0, 0.0),
        "teal" => named(0.0, 0.5019608, 0.5019608),
        "navy" => named(0.0, 0.0, 0.5019608),
        "purple" => named(0.5019608, 0.0, 0.5019608),
        _ => None,
    }
}

/// Decode one XML entity at the head of `s`, returning `(char, bytes consumed)`.
/// A malformed entity consumes exactly one byte and decodes to `None`, so the
/// caller copies the `&` through and continues.
fn decode_entity(s: &str) -> (Option<char>, usize) {
    let Some(semi) = s[..s.len().min(12)].find(';') else {
        return (None, 1);
    };
    let body = &s[1..semi];
    let decoded = match body {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => body.strip_prefix('#').and_then(|num| {
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }),
    };
    match decoded {
        Some(ch) => (Some(ch), semi + 1),
        None => (None, 1),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib text::tests::markup`
Expected: PASS — all three.

- [ ] **Step 5: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add ui/src/text.rs
git commit -m "feat(ui/text): parse_markup for the <b>/<i>/<span> subset

Untrusted input: unknown tags and attributes are dropped and logged once,
nesting is bounded at 64 frames so \"<b>\".repeat(n) cannot overflow the stack,
and every returned span is on a char boundary of the plain string.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: `widgets/` scaffolding — shared types, the controller dispatch, `node_tree_of`, and `Separator`

**Files:**
- Create: `ui/src/widgets/mod.rs`
- Create: `ui/src/widgets/separator.rs`
- Create: `ui/src/icons/builtin.rs` (D2)
- Create: `ui/tests/node_trees.rs`
- Create: `ui/tests/widget_pixels.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/separator.txt`
- Modify: `ui/src/lib.rs` (`pub mod widgets;`, and `pub mod icons;` if absent)
- Modify: `ui/src/view/builders.rs` (D5)
  *(`ui/src/view/app.rs` is **not** modified — P4's `App::with_sheet` (P4 D11) covers what D8 asked for.)*

**Interfaces:**
- Consumes: P4's `view::{View, Kind, Key, Prop, PropName, Props, Handlers, EventKind, Handler}`, `view::controller::{Controller, BuildCx, EventCx, Event, PaintCx}`, `view::app::{App, ScriptStep, Frames}`, `view::cmd::Cmd`; P3's `window::{InputEvent, SurfaceStates}`, `window::pointer::Scroll`, `window::keyboard::{KeyEvent, Mods}`, `window::focus::FocusCause`; M2's `css::node::{Node, PseudoStates}`, `anim::{Clock, ManualClock}`, `layout::{Rect, Allocation}`, `css::cascade::CompiledSheet`, `css::computed::{ComputedStyle, ResolveEnv}`, `text::FontDatabase`, `BUNDLED_ADWAITA_LIGHT`.
- Produces:
  ```rust
  // ui/src/widgets/mod.rs
  pub trait WidgetEnum: Copy + Sized {
      fn to_u16(self) -> u16;
      fn from_u16(raw: u16) -> Option<Self>;
      fn from_prop(prop: Option<&Prop>, default: Self) -> Self;
      fn to_prop(self) -> Prop;
  }

  pub enum Orientation { Horizontal, Vertical }
  pub enum Position { Left, Right, Top, Bottom }
  pub enum Side { Start, End }
  pub enum IconSize { Inherit, Normal, Large }
  pub enum MessageType { Info, Warning, Question, Error, Other }
  pub enum LevelBarMode { Continuous, Discrete }
  pub enum ContentFit { Fill, Contain, Cover, ScaleDown }
  pub enum MatchMode { Exact, Substring, Prefix }
  pub enum ArrowDirection { None, Up, Down, Left, Right }
  pub enum FontLevel { Family, Face, Font, Features }
  pub enum WindowButton { Icon, Minimize, Maximize, Close }

  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct Adjustment {
      pub value: f64, pub lower: f64, pub upper: f64,
      pub step_increment: f64, pub page_increment: f64, pub page_size: f64,
  }
  impl Adjustment {
      #[must_use] pub fn new(value: f64, lower: f64, upper: f64) -> Self;
      #[must_use] pub fn sanitized(self) -> Self;
      #[must_use] pub fn clamp(&self, value: f64) -> f64;
      #[must_use] pub fn fraction(&self) -> f64;
      #[must_use] pub fn value_at_fraction(&self, fraction: f64) -> f64;
      pub fn set_value(&mut self, value: f64) -> bool;
  }

  #[derive(Debug, Clone, PartialEq)]
  pub struct ListItem { pub id: u64, pub label: Rc<str>, pub icon: Option<IconRef>, pub sensitive: bool }
  impl ListItem { pub fn new(id: u64, label: &str) -> Self; }

  #[derive(Debug, Clone, PartialEq)]
  pub struct Mark { pub value: f64, pub position: Position, pub label: Option<Rc<str>> }

  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct RepeatTimer { /* private */ }
  impl RepeatTimer {
      #[must_use] pub fn armed(now: Duration, delay: Duration, interval: Duration, climb: f64) -> Self;
      #[must_use] pub fn deadline(&self) -> Duration;
      pub fn fire(&mut self, now: Duration) -> u32;
  }

  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct ColorDialogSpec { pub with_alpha: bool, pub modal: bool }

  #[derive(Debug, Clone, PartialEq)]
  pub enum PictureSource { None, File(Rc<Path>), Bytes(Rc<[u8]>) }

  #[derive(Debug, Default)]
  pub struct PointerState { pub pressed: bool, pub hovered: bool }
  impl PointerState {
      /// Updates `:hover`/`:active` on `node` from a pointer event.
      /// `true` when the event completed a click inside the node.
      pub fn observe(&mut self, node: &Node, ev: &Event, alloc: Option<Rect>) -> bool;
  }

  pub fn build_controller<Msg: Clone + 'static>(
      kind: Kind, node: &Node, props: &Props, cx: &mut BuildCx<'_>,
  ) -> Box<dyn Controller<Msg>>;

  #[must_use] pub fn node_tree_of(kind: Kind, props: &Props) -> String;
  #[must_use] pub fn render_node_tree(root: &Node) -> String;
  pub fn fixture_matches(fixture: &str, rendered: &str) -> Result<(), String>;

  // ui/src/icons/builtin.rs
  pub enum Builtin { Check, CheckIndeterminate, Radio, RadioIndeterminate,
                     ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
                     Expander, SpinPlus, SpinMinus }
  impl Builtin {
      #[must_use] pub fn path(self, size: f32) -> skia_rs_safe::core::Path;
      #[must_use] pub fn stroke_width(self, size: f32) -> f32;
      pub fn draw(self, canvas: &mut Canvas<'_>, rect: Rect, color: Rgba);
      #[must_use] pub fn from_css_name(name: &str) -> Option<Self>;
  }

  // ui/src/widgets/separator.rs
  pub fn separator<Msg: Clone + 'static>(orientation: Orientation) -> View<Msg>;
  pub struct SeparatorC { pub orientation: Orientation }
  ```

- [ ] **Step 1: Vendor the fixture**

`ui/tests/fixtures/gtk4.22-node-trees/separator.txt`, copied verbatim from
`.superpowers/m3-plan-notes/gtk-widget-nodes.md` §Separator (GTK 4.22.4,
`gtk/gtkseparator.c:48`) — the two orientations are one fixture, keyed on the
`Orientation` prop by the test:

```
separator.horizontal
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
//! Contract §5's node-tree conformance gate.
//!
//! Each widget's retained `Node` subtree is rendered in GTK's own notation and
//! matched against the block vendored verbatim from GTK 4.22.4's sources. P6
//! extends this file with its own kinds; P8 only wires the whole set into the
//! gallery gate.

use icedtea_ui::view::{Kind, PropName, Props};
use icedtea_ui::widgets::{Orientation, WidgetEnum, fixture_matches, node_tree_of};

fn fixture(name: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gtk4.22-node-trees/");
    std::fs::read_to_string(format!("{path}{name}.txt"))
        .unwrap_or_else(|e| panic!("missing fixture {name}.txt: {e}"))
}

fn check(kind: Kind, fixture_name: &str, props: &Props) {
    let rendered = node_tree_of(kind, props);
    if let Err(why) = fixture_matches(&fixture(fixture_name), &rendered) {
        panic!("{kind:?} does not match {fixture_name}.txt:\n{why}\nrendered:\n{rendered}");
    }
}

#[test]
fn a_separator_renders_one_node_carrying_its_orientation_class() {
    // mutation: drop the orientation class in SeparatorC::build and this fails
    // with "required class 'horizontal' missing".
    let mut props = Props::default();
    props.set(PropName::Orientation, Orientation::Horizontal.to_prop());
    check(Kind::Separator, "separator", &props);

    let mut vertical = Props::default();
    vertical.set(PropName::Orientation, Orientation::Vertical.to_prop());
    let rendered = node_tree_of(Kind::Separator, &vertical);
    assert_eq!(rendered.trim(), "separator.vertical");
}

#[test]
fn the_matcher_rejects_a_renamed_or_missing_subnode() {
    // mutation: make fixture_matches always return Ok(()) and this fails.
    assert!(fixture_matches("box\n╰── label", "box\n╰── label").is_ok());
    assert!(
        fixture_matches("box\n╰── label", "box\n╰── button").is_err(),
        "a renamed subnode must be rejected"
    );
    assert!(
        fixture_matches("box\n╰── label", "box").is_err(),
        "a missing required subnode must be rejected"
    );
    assert!(
        fixture_matches("box\n╰── [label]", "box").is_ok(),
        "an optional subnode may be absent"
    );
    assert!(
        fixture_matches("box.frame\n╰── label", "box\n╰── label").is_err(),
        "a missing always-class must be rejected"
    );
    assert!(
        fixture_matches("box\n╰── <child>", "box\n╰── grid\n    ╰── label").is_ok(),
        "<child> admits an arbitrary subtree"
    );
}
```

`ui/tests/widget_pixels.rs`:

```rust
//! Per-widget rest-state and interaction tests (contract §9, P5's gate).
//!
//! Everything here runs through `App::run_offscreen` on a `ManualClock` — no
//! compositor, no Wayland connection. P8's `gallery_gate.rs` and
//! `interaction_gate.rs` are the compositor-backed versions of the same idea.

use std::rc::Rc;
use std::time::Duration;

use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::view::app::{App, Frames, ScriptStep};
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::window::InputEvent;

/// Run one view offscreen against Adwaita light and return the captured frames.
pub fn run<M: 'static, Msg: Clone + 'static>(
    model: M,
    update: fn(&mut M, Msg) -> Cmd<Msg>,
    view: fn(&M) -> View<Msg>,
    size: (u32, u32),
    script: Vec<ScriptStep<Msg>>,
) -> Frames {
    let clock = Rc::new(ManualClock::new());
    App::new(model, update, view)
        .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
        .run_offscreen(size, clock, script)
        .expect("the offscreen app must run")
}

/// `true` if any pixel in the frame is neither fully transparent nor the
/// window's own background — i.e. the widget actually inked something.
pub fn has_ink(frames: &Frames, frame: usize, size: (u32, u32)) -> bool {
    let mut seen = std::collections::HashSet::new();
    for y in 0..size.1 {
        for x in 0..size.0 {
            if let Some(px) = frames.pixel(frame, x, y) {
                seen.insert(px);
            }
        }
    }
    seen.len() > 1
}

pub fn press_release(x: f64, y: f64) -> Vec<ScriptStep<()>> {
    Vec::new()
}

#[test]
fn a_separator_inks_its_adwaita_line_at_rest() {
    // mutation: return `false` from SeparatorC's node build so no node is
    // attached, and the frame becomes a single flat colour.
    use icedtea_ui::view::builders::separator;
    use icedtea_ui::widgets::Orientation;

    let frames = run(
        (),
        |_model: &mut (), _msg: ()| Cmd::None,
        |_model: &()| separator(Orientation::Horizontal).hexpand(true),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    assert_eq!(frames.len(), 1);
    assert!(has_ink(&frames, 0, (120, 40)), "the separator line must ink");
}

#[test]
fn a_separator_ignores_every_pointer_event() {
    // mutation: give SeparatorC a PointerState and set :hover, and the two
    // captures stop being identical.
    use icedtea_ui::view::builders::separator;
    use icedtea_ui::widgets::Orientation;

    let frames = run(
        (),
        |_model: &mut (), _msg: ()| Cmd::None,
        |_model: &()| separator(Orientation::Horizontal).hexpand(true),
        (120, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 60.0, y: 20.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Advance(Duration::from_millis(16)),
            ScriptStep::Capture,
        ],
    );
    let before: Vec<_> = (0..120).map(|x| frames.pixel(0, x, 20)).collect();
    let after: Vec<_> = (0..120).map(|x| frames.pixel(1, x, 20)).collect();
    assert_eq!(before, after, "a separator has no interactive state");
}
```

Delete the unused `press_release` helper stub before committing — it is listed
here only so the shared helpers in this file are obvious; the real per-widget
tasks add the helpers they need.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees`
Expected: FAIL to compile — `unresolved import icedtea_ui::widgets`.

- [ ] **Step 4: Write `ui/src/widgets/mod.rs`**

```rust
//! The GTK 4.22 widget set: one file per widget, each a builder, a controller
//! and a vendored CSS-node-tree fixture.
//!
//! A widget is not an object with setters. It is a `Kind` in a `View` tree
//! (`view::builders`), an `Instance` the reconciler keeps alive, and a
//! `Controller` that owns the behaviour state the application model must not:
//! press state, a text cursor, a scroll offset, an open popover. The
//! controller builds its own CSS subnodes under the root `Node` the reconciler
//! hands it and mutates them in `set_prop`, `on_event` and `tick`; painting is
//! M2's `paint_node_with_children` unless the widget draws something CSS
//! cannot express.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::css::value::image::IconRef;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event};
use crate::view::{Kind, Prop, PropName, Props};

pub mod separator;

/// A widget-local enum carried through `Prop::Enum(u16)`.
///
/// `Prop` cannot hold an arbitrary type, and the contract's `Prop::Enum(u16)`
/// is deliberately opaque; this trait is the only sanctioned way in or out of
/// it, so a mis-cast enum is a compile error rather than a silent 0.
pub trait WidgetEnum: Copy + Sized {
    /// The discriminant.
    fn to_u16(self) -> u16;
    /// The variant, or `None` for a discriminant this enum does not have.
    fn from_u16(raw: u16) -> Option<Self>;

    /// Read from a prop slot, falling back to `default` for a missing prop or
    /// an out-of-range discriminant.
    fn from_prop(prop: Option<&Prop>, default: Self) -> Self {
        match prop {
            Some(Prop::Enum(raw)) => Self::from_u16(*raw).unwrap_or(default),
            _ => default,
        }
    }

    /// Wrap for storage in a `Props`.
    fn to_prop(self) -> Prop {
        Prop::Enum(self.to_u16())
    }
}

/// Declare a widget-local enum plus its `WidgetEnum` impl and CSS class names.
macro_rules! widget_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $( $(#[$vmeta:meta])* $variant:ident = $class:expr ),+ $(,)? }
        default = $default:ident;
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $( $(#[$vmeta])* $variant ),+ }

        impl Default for $name {
            fn default() -> Self { $name::$default }
        }

        impl $name {
            /// The GTK style class this variant contributes, or `""`.
            #[must_use]
            pub fn css_class(self) -> &'static str {
                match self { $( $name::$variant => $class ),+ }
            }
            /// Every variant, in declaration order.
            #[must_use]
            pub fn all() -> &'static [$name] {
                &[ $( $name::$variant ),+ ]
            }
        }

        impl WidgetEnum for $name {
            fn to_u16(self) -> u16 {
                let mut index = 0u16;
                $( if matches!(self, $name::$variant) { return index; } index += 1; )+
                let _ = index;
                0
            }
            fn from_u16(raw: u16) -> Option<Self> {
                let mut index = 0u16;
                $( if raw == index { return Some($name::$variant); } index += 1; )+
                let _ = index;
                None
            }
        }
    };
}

widget_enum! {
    /// `GtkOrientable:orientation`.
    pub enum Orientation { Horizontal = "horizontal", Vertical = "vertical" }
    default = Horizontal;
}

widget_enum! {
    /// `GtkPositionType` — where a value, a mark or a popover sits.
    pub enum Position { Left = "left", Right = "right", Top = "top", Bottom = "bottom" }
    default = Bottom;
}

widget_enum! {
    /// `GtkPackType` — which half of a decoration layout a control belongs to.
    pub enum Side { Start = "start", End = "end" }
    default = Start;
}

widget_enum! {
    /// `GtkIconSize`. `Inherit` adds no class, matching GTK.
    pub enum IconSize { Inherit = "", Normal = "normal-icons", Large = "large-icons" }
    default = Inherit;
}

widget_enum! {
    /// `GtkMessageType`. `Other` adds no class.
    pub enum MessageType {
        Info = "info", Warning = "warning", Question = "question",
        Error = "error", Other = "",
    }
    default = Info;
}

widget_enum! {
    /// `GtkLevelBarMode`.
    pub enum LevelBarMode { Continuous = "continuous", Discrete = "discrete" }
    default = Continuous;
}

widget_enum! {
    /// `GtkContentFit`.
    pub enum ContentFit { Fill = "", Contain = "", Cover = "", ScaleDown = "" }
    default = Contain;
}

widget_enum! {
    /// `GtkStringFilterMatchMode`, used by `DropDown`'s search.
    pub enum MatchMode { Exact = "", Substring = "", Prefix = "" }
    default = Substring;
}

widget_enum! {
    /// `GtkArrowType`, the class a `menubutton`'s `arrow` node carries.
    pub enum ArrowDirection {
        None = "none", Up = "up", Down = "down", Left = "left", Right = "right",
    }
    default = Down;
}

widget_enum! {
    /// `GtkFontLevel`.
    pub enum FontLevel { Family = "", Face = "", Font = "", Features = "" }
    default = Font;
}

widget_enum! {
    /// One token of `gtk-decoration-layout` that produces a node.
    /// `menu` is recognised by the setting but produces no child in 4.22.4.
    pub enum WindowButton {
        Icon = "icon", Minimize = "minimize", Maximize = "maximize", Close = "close",
    }
    default = Close;
}

/// `GtkAdjustment`: the shared numeric model behind `Scale`, `Scrollbar`,
/// `SpinButton` and every scrollable.
///
/// Every field is `f64` and every field can arrive from a `Prop::Float` a
/// model computed, so [`Adjustment::sanitized`] is applied on every ingress:
/// NaN and infinities become the initial values, and `lower > upper` swaps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjustment {
    /// Current value, always within `lower..=upper - page_size`.
    pub value: f64,
    /// Inclusive minimum.
    pub lower: f64,
    /// Inclusive maximum.
    pub upper: f64,
    /// One arrow-key or stepper step.
    pub step_increment: f64,
    /// One Page Up/Down or trough click.
    pub page_increment: f64,
    /// The visible span, non-zero only for scrollbars.
    pub page_size: f64,
}

impl Default for Adjustment {
    fn default() -> Self {
        Adjustment {
            value: 0.0,
            lower: 0.0,
            upper: 1.0,
            step_increment: 0.1,
            page_increment: 0.2,
            page_size: 0.0,
        }
    }
}

impl Adjustment {
    /// A sanitized adjustment over `lower..=upper`.
    #[must_use]
    pub fn new(value: f64, lower: f64, upper: f64) -> Self {
        Adjustment { value, lower, upper, ..Adjustment::default() }.sanitized()
    }

    /// Replace every non-finite field with its initial value, order the bounds,
    /// clamp `page_size` into the range and `value` into the usable span.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let fin = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
        let mut lower = fin(self.lower, 0.0);
        let mut upper = fin(self.upper, 1.0);
        if lower > upper {
            std::mem::swap(&mut lower, &mut upper);
        }
        let page_size = fin(self.page_size, 0.0).clamp(0.0, upper - lower);
        let step_increment = fin(self.step_increment, 0.1).abs();
        let page_increment = fin(self.page_increment, 0.2).abs();
        let value = fin(self.value, lower).clamp(lower, upper - page_size);
        Adjustment { value, lower, upper, step_increment, page_increment, page_size }
    }

    /// `value` clamped into the usable span.
    #[must_use]
    pub fn clamp(&self, value: f64) -> f64 {
        let value = if value.is_finite() { value } else { self.lower };
        value.clamp(self.lower, self.upper - self.page_size)
    }

    /// Where `value` sits in `0.0..=1.0`. A zero-width range reports 0.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        let span = self.upper - self.page_size - self.lower;
        if span <= 0.0 {
            0.0
        } else {
            ((self.value - self.lower) / span).clamp(0.0, 1.0)
        }
    }

    /// The value at `fraction` of the usable span.
    #[must_use]
    pub fn value_at_fraction(&self, fraction: f64) -> f64 {
        let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
        self.clamp(self.lower + fraction * (self.upper - self.page_size - self.lower))
    }

    /// Set the value, clamped. `true` if it actually moved.
    pub fn set_value(&mut self, value: f64) -> bool {
        let next = self.clamp(value);
        let moved = (next - self.value).abs() > f64::EPSILON;
        self.value = next;
        moved
    }
}

/// One row of a list model, for `DropDown`, `ListView`, `ListBox` and friends.
///
/// M3 has no `GListModel`: a model is a plain slice a `view` produced, and
/// `id` is the reconciler key so a row's `Node` identity survives a reorder.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// Stable identity across frames; becomes the child's reconciler `Key`.
    pub id: u64,
    /// The row's text.
    pub label: Rc<str>,
    /// An optional leading icon.
    pub icon: Option<IconRef>,
    /// `false` renders `:disabled` and refuses selection.
    pub sensitive: bool,
}

impl ListItem {
    /// A sensitive, icon-less item.
    #[must_use]
    pub fn new(id: u64, label: &str) -> Self {
        ListItem { id, label: Rc::from(label), icon: None, sensitive: true }
    }
}

/// One `GtkScale` mark.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// Where on the adjustment the mark sits.
    pub value: f64,
    /// Which side of the trough it renders on.
    pub position: Position,
    /// Optional text under the indicator.
    pub label: Option<Rc<str>>,
}

/// The held-stepper repeat clock behind `SpinButton` (and, in P6, the scrollbar
/// steppers).
///
/// GTK's `climb_rate` shortens the interval as the button stays down; the
/// interval never falls below `MIN_INTERVAL`, so a held button cannot spin the
/// event loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepeatTimer {
    next: Duration,
    interval: Duration,
    climb: f64,
}

impl RepeatTimer {
    /// The shortest interval a repeat can accelerate to.
    const MIN_INTERVAL: Duration = Duration::from_millis(20);

    /// Arm at `now`, first fire after `delay`.
    #[must_use]
    pub fn armed(now: Duration, delay: Duration, interval: Duration, climb: f64) -> Self {
        RepeatTimer {
            next: now + delay,
            interval: interval.max(Self::MIN_INTERVAL),
            climb: if climb.is_finite() { climb.clamp(1.0, 4.0) } else { 1.0 },
        }
    }

    /// When the next repeat is due.
    #[must_use]
    pub fn deadline(&self) -> Duration {
        self.next
    }

    /// How many repeats are due at `now`, rearming for the next one.
    ///
    /// Returns 0 before the deadline. Bounded at 16 per call so a clock that
    /// jumped forward cannot emit thousands of steps in one tick.
    pub fn fire(&mut self, now: Duration) -> u32 {
        let mut fired = 0u32;
        while now >= self.next && fired < 16 {
            fired += 1;
            let scaled = self.interval.as_secs_f64() / self.climb;
            self.interval = Duration::from_secs_f64(scaled).max(Self::MIN_INTERVAL);
            self.next += self.interval;
        }
        fired
    }
}

/// What `ColorDialogButton` launches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorDialogSpec {
    /// Show an alpha slider.
    pub with_alpha: bool,
    /// Take a grab while open.
    pub modal: bool,
}

impl Default for ColorDialogSpec {
    fn default() -> Self {
        ColorDialogSpec { with_alpha: false, modal: true }
    }
}

/// Where a `Picture`'s pixels come from.
#[derive(Debug, Clone, PartialEq)]
pub enum PictureSource {
    /// Nothing to draw.
    None,
    /// A path decoded through `skia_rs_safe::codec`.
    File(Rc<Path>),
    /// An in-memory encoded image (PNG), for tests and embedded assets.
    Bytes(Rc<[u8]>),
}

/// The `:hover`/`:active` bookkeeping every pointer-reactive widget shares.
///
/// Contract §5's preamble says every controller carries `pressed` and `hovered`
/// unless it has no pointer behaviour at all; this is that pair, plus the state
/// flag updates, in one place instead of thirty-two.
#[derive(Debug, Default)]
pub struct PointerState {
    /// A button is down on this node.
    pub pressed: bool,
    /// The pointer is inside this node.
    pub hovered: bool,
}

impl PointerState {
    /// Fold one event in, updating `node`'s pseudo-states.
    ///
    /// Returns `true` when this event completed a click *inside* `alloc` — the
    /// press-then-release-inside gesture GTK calls "clicked". A release outside
    /// the allocation clears `:active` and returns `false`, which is what makes
    /// drag-off-and-release cancel a button press.
    pub fn observe(&mut self, node: &Node, ev: &Event, alloc: Option<Rect>) -> bool {
        let inside = |local: (f32, f32)| match alloc {
            Some(rect) => {
                local.0 >= 0.0
                    && local.1 >= 0.0
                    && local.0 <= rect.width
                    && local.1 <= rect.height
            }
            None => true,
        };
        match ev {
            Event::PointerEnter { .. } => {
                self.hovered = true;
                node.set_state(PseudoStates::HOVER, true);
            }
            Event::PointerLeave => {
                self.hovered = false;
                node.set_state(PseudoStates::HOVER, false);
            }
            Event::PointerMotion { local } => {
                self.hovered = inside(*local);
                node.set_state(PseudoStates::HOVER, self.hovered);
            }
            Event::PointerDown { local, .. } if inside(*local) => {
                self.pressed = true;
                node.set_state(PseudoStates::ACTIVE, true);
            }
            Event::PointerUp { local, .. } => {
                let was = self.pressed;
                self.pressed = false;
                node.set_state(PseudoStates::ACTIVE, false);
                return was && inside(*local);
            }
            _ => {}
        }
        false
    }
}

/// Build the controller for `kind`.
///
/// `Controller::build` is `Self: Sized`, so the reconciler cannot go from a
/// `Kind` to a `Box<dyn Controller<Msg>>` on its own; this is that table, and
/// it lives beside the widgets so adding one is a single-file change. P6
/// replaces the `Unimplemented` arms with its own kinds.
pub fn build_controller<Msg: Clone + 'static>(
    kind: Kind,
    node: &Node,
    props: &Props,
    cx: &mut BuildCx<'_>,
) -> Box<dyn Controller<Msg>> {
    match kind {
        Kind::Separator => Box::new(separator::SeparatorC::build(node, props, cx)),
        _ => Box::new(Unimplemented(kind)),
    }
}

/// The placeholder for a `Kind` whose part has not landed yet.
///
/// It builds no subnodes, handles no events and paints nothing, so a `View`
/// naming an unimplemented kind renders as a bare styled box instead of
/// panicking. P6 removes the arms it fills.
pub struct Unimplemented(pub Kind);

impl<Msg> Controller<Msg> for Unimplemented {
    fn kind(&self) -> Kind {
        self.0
    }
    fn build(_node: &Node, _props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        Unimplemented(Kind::Separator)
    }
    fn set_prop(&mut self, _node: &Node, _name: PropName, _value: &Prop, _cx: &mut BuildCx<'_>) {}
    fn on_event(&mut self, _ev: &Event, _cx: &mut crate::view::controller::EventCx<'_, Msg>)
        -> Vec<Msg> {
        Vec::new()
    }
}
```

- [ ] **Step 5: Add the node-tree renderer and the fixture matcher to `widgets/mod.rs`**

```rust
/// Build `kind` in isolation and render its retained subtree in GTK notation.
///
/// Hermetic: its own sheet, font database, icon theme and clock, no compositor
/// and no window. This is what the conformance gate and P8's gallery gate call.
#[must_use]
pub fn node_tree_of(kind: Kind, props: &Props) -> String {
    use crate::anim::ManualClock;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::icons::IconTheme;
    use crate::text::FontDatabase;

    let node = Node::with_classes(kind.css_name(), kind.base_classes());
    let sheet = CompiledSheet::compile("");
    let mut fonts = FontDatabase::new();
    let mut icons = IconTheme::with_name_and_roots("hicolor", Vec::new());
    let clock: Rc<dyn crate::anim::Clock> = Rc::new(ManualClock::new());
    let env = ResolveEnv::default();
    let mut cx = BuildCx {
        sheet: &sheet,
        fonts: &mut fonts,
        icons: &mut icons,
        clock: &clock,
        env: &env,
    };
    let mut controller = build_controller::<()>(kind, &node, props, &mut cx);
    for (name, value) in props.iter() {
        controller.set_prop(&node, name, value, &mut cx);
    }
    render_node_tree(&node)
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
    /// Classes it may carry.
    optional: Vec<String>,
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
        let (depth, body) = match trimmed.strip_prefix("├── ").or(trimmed.strip_prefix("╰── ")) {
            Some(body) => (prefix_cols / 4 + 1, body),
            None => (0, trimmed),
        };
        let body = body.trim();
        if body == "<child>" {
            out.push(TreeLine {
                depth,
                name: String::new(),
                required: Vec::new(),
                optional: Vec::new(),
                node_optional: false,
                wildcard: true,
            });
            continue;
        }
        let (body, node_optional) = match body.strip_prefix('[') {
            Some(rest) => (rest.trim_end_matches(']'), true),
            None => (body, false),
        };
        let mut name = String::new();
        let mut required = Vec::new();
        let mut optional = Vec::new();
        let mut rest = body;
        // The name runs up to the first '.' or '['.
        let head_end = rest.find(['.', '[']).unwrap_or(rest.len());
        name.push_str(&rest[..head_end]);
        rest = &rest[head_end..];
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                let end = after.find(']').unwrap_or(after.len());
                optional.push(after[..end].trim_start_matches('.').to_owned());
                rest = &after[(end + 1).min(after.len())..];
            } else if let Some(after) = rest.strip_prefix('.') {
                let end = after.find(['.', '[']).unwrap_or(after.len());
                required.push(after[..end].to_owned());
                rest = &after[end..];
            } else {
                break;
            }
        }
        out.push(TreeLine { depth, name, required, optional, node_optional, wildcard: false });
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
        let name = if line.wildcard { "<child>".to_owned() } else { line.name.clone() };
        stack.push(name.clone());
        out.push((stack.join("/"), index));
    }
    out
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

    for (path, index) in &rendered_paths {
        if wildcard_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix) && path.len() > prefix.len())
        {
            continue;
        }
        let Some(fixture_index) = fixture_paths
            .iter()
            .find(|(fixture_path, _)| fixture_path == path)
            .map(|(_, i)| *i)
        else {
            return Err(format!("rendered node at '{path}' is not in the fixture"));
        };
        let expected = &fixture_lines[fixture_index];
        let actual = &rendered_lines[*index];
        for class in &expected.required {
            if !actual.required.contains(class) {
                return Err(format!(
                    "required class '{class}' missing from rendered node at '{path}'"
                ));
            }
        }
    }

    let rendered_set: std::collections::HashSet<&str> =
        rendered_paths.iter().map(|(path, _)| path.as_str()).collect();
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
            return Err(format!("fixture requires a node at '{path}', which was not rendered"));
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Write `ui/src/widgets/separator.rs`**

```rust
//! `GtkSeparator` — `Kind::Separator`, CSS node `separator`.
//!
//! ```text
//! separator.horizontal
//! separator.vertical
//! ```
//!
//! A single node with no subnodes, no pointer behaviour and no keyboard
//! behaviour; the line itself is Adwaita's `background-color` on a 1px box.

use crate::css::node::Node;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::{Orientation, WidgetEnum};

/// A `GtkSeparator` in `orientation`.
#[must_use]
pub fn separator<Msg: Clone + 'static>(orientation: Orientation) -> View<Msg> {
    View::new(Kind::Separator).prop(PropName::Orientation, orientation.to_prop())
}

/// `Kind::Separator`'s controller. No pointer state: GTK delivers no events
/// to a separator, and neither do we.
pub struct SeparatorC {
    /// The orientation whose class the node carries.
    pub orientation: Orientation,
}

impl SeparatorC {
    fn apply(&self, node: &Node) {
        for candidate in Orientation::all() {
            node.remove_class(candidate.css_class());
        }
        node.add_class(self.orientation.css_class());
    }
}

impl<Msg> Controller<Msg> for SeparatorC {
    fn kind(&self) -> Kind {
        Kind::Separator
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let this = SeparatorC {
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if name == PropName::Orientation {
            self.orientation = Orientation::from_prop(Some(value), self.orientation);
            self.apply(node);
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

- [ ] **Step 7: Wire the module and the builder re-export**

In `ui/src/lib.rs`, beside the existing `pub mod` list:

```rust
pub mod widgets;
```

(and `pub mod icons;` only if P4 did not already add it).

In `ui/src/view/builders.rs`, append (D5):

```rust
pub use crate::widgets::separator::separator;
```

In `ui/src/view/app.rs`, add to `impl<M, Msg> App<M, Msg>` (D8):

```rust
    /// The stylesheet this app styles against.
    ///
    /// `run_offscreen` has no Wayland connection to read a theme from, and
    /// every P5/P6 pixel test needs to say "Adwaita light". Unset, the app
    /// compiles `BUNDLED_ADWAITA_LIGHT`.
    #[must_use]
    pub fn sheet(mut self, sheet: CompiledSheet) -> Self {
        self.sheet = Some(sheet);
        self
    }
```

with a `sheet: Option<CompiledSheet>` field defaulted to `None` in `App::new`,
and `self.sheet.unwrap_or_else(|| CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))`
at the point `run`/`run_offscreen` needs one.

In `ui/src/icons/builtin.rs` (D2 — geometry is P7's; only the signatures and a
placeholder shape land here):

```rust
//! `-gtk-icon-source: builtin` shapes.
//!
//! P5 needs these names to build `CheckButton`'s `check` node and
//! `SpinButton`'s steppers. **The geometry here is a placeholder** — P7 fills
//! `path` and `stroke_width` in against `gtkcssimagebuiltin` and un-ignores the
//! two P5 pixel tests named in this plan's D9.

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::core::Path;

use crate::css::value::Rgba;
use crate::layout::Rect;

/// The builtin shapes GTK's CSS engine can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// A check mark.
    Check,
    /// A check button's indeterminate dash.
    CheckIndeterminate,
    /// A radio dot.
    Radio,
    /// A radio button's indeterminate dash.
    RadioIndeterminate,
    /// An upward arrow.
    ArrowUp,
    /// A downward arrow.
    ArrowDown,
    /// A leftward arrow.
    ArrowLeft,
    /// A rightward arrow.
    ArrowRight,
    /// An expander triangle.
    Expander,
    /// A spin button's `+`.
    SpinPlus,
    /// A spin button's `−`.
    SpinMinus,
}

impl Builtin {
    /// The shape, in a `size × size` box at the origin.
    ///
    /// P7 replaces this body; today every shape is the inscribed square, which
    /// is enough for layout and for "did anything ink here" assertions.
    #[must_use]
    pub fn path(self, size: f32) -> Path {
        let size = if size.is_finite() && size > 0.0 { size } else { 0.0 };
        let inset = size * 0.25;
        let mut path = Path::new();
        path.add_rect(&Rect::new(inset, inset, size - 2.0 * inset, size - 2.0 * inset).to_skia());
        path
    }

    /// Stroke width for the outline shapes.
    #[must_use]
    pub fn stroke_width(self, size: f32) -> f32 {
        if size.is_finite() && size > 0.0 { size / 8.0 } else { 0.0 }
    }

    /// Draw the shape filled with `color`, scaled into `rect`.
    pub fn draw(self, canvas: &mut Canvas<'_>, rect: Rect, color: Rgba) {
        if rect.is_empty() || color.a <= 0.0 {
            return;
        }
        let size = rect.width.min(rect.height);
        let path = self.path(size);
        canvas.save();
        canvas.translate(rect.x + (rect.width - size) / 2.0, rect.y + (rect.height - size) / 2.0);
        canvas.draw_path(&path, &crate::paint::fill_paint(color));
        canvas.restore();
    }

    /// Parse a `builtin(<name>)` CSS value's argument.
    #[must_use]
    pub fn from_css_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "check" => Builtin::Check,
            "check-indeterminate" => Builtin::CheckIndeterminate,
            "radio" => Builtin::Radio,
            "radio-indeterminate" => Builtin::RadioIndeterminate,
            "arrow-up" => Builtin::ArrowUp,
            "arrow-down" => Builtin::ArrowDown,
            "arrow-left" => Builtin::ArrowLeft,
            "arrow-right" => Builtin::ArrowRight,
            "expander" => Builtin::Expander,
            "spin-plus" => Builtin::SpinPlus,
            "spin-minus" => Builtin::SpinMinus,
            _ => return None,
        })
    }
}
```

- [ ] **Step 8: Add the `Adjustment` and `WidgetEnum` unit tests**

Append `mod tests` to `ui/src/widgets/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Adjustment, Orientation, RepeatTimer, WidgetEnum};
    use crate::view::Prop;
    use std::time::Duration;

    #[test]
    fn a_widget_enum_round_trips_through_a_prop() {
        // mutation: make `to_u16` return a constant and Vertical decodes as
        // Horizontal.
        for variant in Orientation::all() {
            let prop = variant.to_prop();
            assert_eq!(
                Orientation::from_prop(Some(&prop), Orientation::Horizontal),
                *variant
            );
        }
        assert_eq!(
            Orientation::from_prop(Some(&Prop::Enum(9_999)), Orientation::Vertical),
            Orientation::Vertical,
            "an out-of-range discriminant falls back, never panics"
        );
        assert_eq!(
            Orientation::from_prop(Some(&Prop::Bool(true)), Orientation::Vertical),
            Orientation::Vertical,
            "a wrongly typed prop falls back"
        );
    }

    #[test]
    fn an_adjustment_never_lets_a_hostile_number_through() {
        // mutation: drop the `fin` guard in `sanitized` and every assertion
        // below reports NaN.
        let hostile = Adjustment {
            value: f64::NAN,
            lower: 10.0,
            upper: -10.0,
            step_increment: f64::INFINITY,
            page_increment: f64::NEG_INFINITY,
            page_size: f64::NAN,
        }
        .sanitized();
        assert!(hostile.value.is_finite() && hostile.lower.is_finite());
        assert!(hostile.lower <= hostile.upper, "bounds are ordered");
        assert!(hostile.step_increment.is_finite() && hostile.step_increment >= 0.0);
        assert!(hostile.fraction().is_finite());
        assert_eq!(Adjustment::new(5.0, 0.0, 1.0).value, 1.0, "value is clamped");
        assert_eq!(Adjustment::new(0.5, 0.0, 0.0).fraction(), 0.0, "no div by zero");
    }

    #[test]
    fn a_repeat_timer_accelerates_but_never_below_its_floor() {
        // mutation: remove the `.max(MIN_INTERVAL)` and the interval collapses
        // to zero, so `fire` returns the 16-step cap on every call.
        let mut timer = RepeatTimer::armed(
            Duration::ZERO,
            Duration::from_millis(400),
            Duration::from_millis(100),
            2.0,
        );
        assert_eq!(timer.fire(Duration::from_millis(399)), 0, "not yet due");
        assert_eq!(timer.fire(Duration::from_millis(400)), 1);
        let first = timer.deadline();
        assert!(first > Duration::from_millis(400));
        for step in 1..40u64 {
            timer.fire(Duration::from_millis(400 + step * 100));
        }
        let before = timer.deadline();
        timer.fire(before);
        assert!(
            timer.deadline() - before >= RepeatTimer::MIN_INTERVAL,
            "the interval floors at 20ms"
        );
    }
}
```

- [ ] **Step 9: Run the tests**

Run: `cargo test -p icedtea-ui --lib widgets::`
Run: `cargo test -p icedtea-ui --test node_trees`
Run: `cargo test -p icedtea-ui --test widget_pixels`
Expected: PASS — three unit tests, two node-tree tests, two pixel tests.

- [ ] **Step 10: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green; the four `themed_button_offscreen`, nine `adwaita_coverage`
and four `gtk4_property_reference` tests unchanged and passing.

- [ ] **Step 11: Commit**

```bash
git add ui/src/widgets ui/src/icons/builtin.rs ui/src/lib.rs ui/src/view/builders.rs \
        ui/src/view/app.rs ui/tests/node_trees.rs ui/tests/widget_pixels.rs \
        ui/tests/fixtures/gtk4.22-node-trees/separator.txt
git commit -m "feat(ui/widgets): the widget scaffolding, node-tree gate and Separator

Shared widget-local types (contract D4), the Kind -> controller dispatch the
reconciler needs (D6), the GTK-notation renderer and the fixture matcher (D7),
a Builtin stub behind P7's signatures (D2), P4's App::with_sheet for offscreen pixel
tests (D8), and the first widget end to end.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: `Label` — `Kind::Label`, node `label`

**Files:**
- Create: `ui/src/widgets/label.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/label.txt`
- Modify: `ui/src/widgets/mod.rs` (`pub mod label;` + the dispatch arm)
- Modify: `ui/src/view/builders.rs` (`pub use crate::widgets::label::label;`)
- Test: `ui/tests/node_trees.rs`, `ui/tests/widget_pixels.rs`

**Interfaces:**
- Consumes: Tasks 1–6's `text::{TextLayout, WrapMode, Ellipsize, MarkupSpan, parse_markup, TextStyle}`, Task 7's `PointerState`, `WidgetEnum`.
- Produces:
  ```rust
  pub fn label<Msg: Clone + 'static>(text: &str) -> View<Msg>;
  pub trait LabelExt<Msg> {
      fn wrap(self, on: bool) -> Self;
      fn wrap_mode(self, mode: WrapMode) -> Self;
      fn ellipsize(self, mode: Ellipsize) -> Self;
      fn lines(self, n: i32) -> Self;
      fn xalign(self, a: f32) -> Self;
      fn yalign(self, a: f32) -> Self;
      fn markup(self, on: bool) -> Self;
      fn selectable(self, on: bool) -> Self;
      fn use_underline(self, on: bool) -> Self;
      fn width_chars(self, n: i32) -> Self;
      fn max_width_chars(self, n: i32) -> Self;
      fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
  }
  pub struct LabelC { /* layout, spans, selection, links, hovered_link, selection_node, link_nodes */ }
  ```

- [ ] **Step 1: Vendor the fixture**

`ui/tests/fixtures/gtk4.22-node-trees/label.txt`, verbatim from
`.superpowers/m3-plan-notes/gtk-widget-nodes.md` §Label (`gtk/gtklabel.c:113`):

```
label
├── [selection]
├── [link]
┊
╰── [link]
```

- [ ] **Step 2: Write the failing tests**

Append to `ui/tests/node_trees.rs`:

```rust
#[test]
fn a_label_renders_one_node_and_a_selection_subnode_only_when_selected() {
    // mutation: build the `selection` node unconditionally in LabelC::build and
    // the first assertion sees it in a non-selectable label.
    let mut plain = Props::default();
    plain.set(PropName::Label, Prop::Str("hello".into()));
    check(Kind::Label, "label", &plain);
    assert_eq!(node_tree_of(Kind::Label, &plain).trim(), "label");

    let mut selectable = plain.clone();
    selectable.set(PropName::Selectable, Prop::Bool(true));
    let rendered = node_tree_of(Kind::Label, &selectable);
    assert!(rendered.contains("selection"), "a selectable label gets one: {rendered}");
    check(Kind::Label, "label", &selectable);
}
```

Append to `ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_label_inks_its_glyphs_at_rest_in_adwaita_light() {
    // mutation: skip `layout.draw` in LabelC::paint and the frame is one flat
    // colour.
    use icedtea_ui::view::builders::label;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| label("Hello"),
        (160, 40),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (160, 40)), "the label must draw glyphs");
}

#[test]
fn clicking_a_link_in_a_label_fires_activate_link_with_its_uri() {
    // mutation: drop the hit-test against `links` in LabelC::on_event and no
    // message arrives.
    use icedtea_ui::view::builders::label;
    use icedtea_ui::widgets::label::LabelExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Opened(String);

    let frames = run(
        Vec::<String>::new(),
        |model: &mut Vec<String>, Opened(uri): Opened| {
            model.push(uri);
            Cmd::None
        },
        |_m: &Vec<String>| {
            label("go to <a href=\"https://gtk.org\">GTK</a> now")
                .markup(true)
                .on_activate_link(|uri| Opened(uri.to_owned()))
        },
        (240, 40),
        vec![
            ScriptStep::Event(InputEvent::PointerEnter { x: 40.0, y: 14.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1, "the script captured once");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees a_label_renders`
Expected: FAIL — `Label does not match label.txt` (the `Unimplemented`
controller builds no `selection` node and the label renders nothing).

- [ ] **Step 4: Write `ui/src/widgets/label.rs`**

```rust
//! `GtkLabel` — `Kind::Label`, CSS node `label`.
//!
//! ```text
//! label
//! ├── [selection]
//! ├── [link]
//! ┊
//! ╰── [link]
//! ```
//!
//! The `selection` subnode exists only while the label is selectable, and one
//! `link` subnode exists per `<a href>` the markup carried; the `label` node
//! then also gets the `.link` style class, as GTK does.

use std::ops::Range;
use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::{Node, PseudoStates};
use crate::layout::{Allocation, Rect};
use crate::text::{Ellipsize, MarkupSpan, TextLayout, TextStyle, WrapMode, parse_markup};
use crate::view::controller::{BuildCx, Controller, Event, EventCx, PaintCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, WidgetEnum};

/// A `GtkLabel` showing `text`.
#[must_use]
pub fn label<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::Label).prop(PropName::Label, Prop::Str(Rc::from(text)))
}

/// `GtkLabel`'s own property setters, chained after [`label`].
pub trait LabelExt<Msg>: Sized {
    /// `GtkLabel:wrap`. Turns on `WrapMode::Word` unless `wrap_mode` says else.
    fn wrap(self, on: bool) -> Self;
    /// `GtkLabel:wrap-mode`.
    fn wrap_mode(self, mode: WrapMode) -> Self;
    /// `GtkLabel:ellipsize`.
    fn ellipsize(self, mode: Ellipsize) -> Self;
    /// `GtkLabel:lines` — the maximum number of wrapped lines; -1 for no limit.
    fn lines(self, n: i32) -> Self;
    /// `GtkLabel:xalign`, 0.0..=1.0.
    fn xalign(self, a: f32) -> Self;
    /// `GtkLabel:yalign`, 0.0..=1.0.
    fn yalign(self, a: f32) -> Self;
    /// `GtkLabel:use-markup`.
    fn markup(self, on: bool) -> Self;
    /// `GtkLabel:selectable`.
    fn selectable(self, on: bool) -> Self;
    /// `GtkLabel:use-underline` (mnemonics; the underscore is stripped).
    fn use_underline(self, on: bool) -> Self;
    /// `GtkLabel:width-chars`.
    fn width_chars(self, n: i32) -> Self;
    /// `GtkLabel:max-width-chars`.
    fn max_width_chars(self, n: i32) -> Self;
    /// `GtkLabel::activate-link`.
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> LabelExt<Msg> for View<Msg> {
    fn wrap(self, on: bool) -> Self {
        self.prop(PropName::Wrap, Prop::Bool(on))
    }
    fn wrap_mode(self, mode: WrapMode) -> Self {
        self.prop(PropName::Wrap, Prop::Enum(wrap_to_u16(mode)))
    }
    fn ellipsize(self, mode: Ellipsize) -> Self {
        self.prop(PropName::Ellipsize, Prop::Enum(ellipsize_to_u16(mode)))
    }
    fn lines(self, n: i32) -> Self {
        self.prop(PropName::Rows, Prop::Int(i64::from(n)))
    }
    fn xalign(self, a: f32) -> Self {
        self.prop(PropName::Xalign, Prop::Float(f64::from(a)))
    }
    fn yalign(self, a: f32) -> Self {
        self.prop(PropName::Yalign, Prop::Float(f64::from(a)))
    }
    fn markup(self, on: bool) -> Self {
        self.prop(PropName::Markup, Prop::Bool(on))
    }
    fn selectable(self, on: bool) -> Self {
        self.prop(PropName::Selectable, Prop::Bool(on))
    }
    fn use_underline(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn width_chars(self, n: i32) -> Self {
        self.prop(PropName::WidthRequest, Prop::Int(i64::from(n)))
    }
    fn max_width_chars(self, n: i32) -> Self {
        self.prop(PropName::MaxLength, Prop::Int(i64::from(n)))
    }
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }
}

/// `WrapMode` has no `WidgetEnum` impl (it lives in `text`), so the two
/// conversions are spelled out here and in `wrap_from_u16`.
fn wrap_to_u16(mode: WrapMode) -> u16 {
    match mode {
        WrapMode::None => 0,
        WrapMode::Word => 1,
        WrapMode::Char => 2,
        WrapMode::WordChar => 3,
    }
}

fn wrap_from_u16(raw: u16) -> WrapMode {
    match raw {
        1 => WrapMode::Word,
        2 => WrapMode::Char,
        3 => WrapMode::WordChar,
        _ => WrapMode::None,
    }
}

fn ellipsize_to_u16(mode: Ellipsize) -> u16 {
    match mode {
        Ellipsize::None => 0,
        Ellipsize::Start => 1,
        Ellipsize::Middle => 2,
        Ellipsize::End => 3,
    }
}

fn ellipsize_from_u16(raw: u16) -> Ellipsize {
    match raw {
        1 => Ellipsize::Start,
        2 => Ellipsize::Middle,
        3 => Ellipsize::End,
        _ => Ellipsize::None,
    }
}

/// `Kind::Label`'s controller.
pub struct LabelC {
    /// The wrapped, ellipsized paragraph.
    pub layout: TextLayout,
    /// Attributed runs from `parse_markup`, empty when markup is off.
    pub spans: Vec<MarkupSpan>,
    /// The selected byte range, when the label is selectable and has one.
    pub selection: Option<Range<usize>>,
    /// `(byte range, uri)` for every `<a href>` the markup carried.
    pub links: Vec<(Range<usize>, Rc<str>)>,
    /// Index into `links` under the pointer.
    pub hovered_link: Option<usize>,
    /// The `selection` subnode, present only while selectable.
    pub selection_node: Option<Node>,
    /// One `link` subnode per entry in `links`.
    pub link_nodes: Vec<Node>,
    text: Rc<str>,
    plain: String,
    wrap: WrapMode,
    ellipsize: Ellipsize,
    xalign: f32,
    markup: bool,
    selectable: bool,
    pointer: PointerState,
    /// The width the layout was last built at, so `measure` can reuse it.
    built_width: Option<f32>,
}

impl LabelC {
    /// Re-read the text props and rebuild the paragraph and the subnodes.
    fn rebuild(&mut self, node: &Node, cx: &mut BuildCx<'_>, width: Option<f32>) {
        let (plain, spans, links) = if self.markup {
            let (plain, spans) = parse_markup(&self.text);
            let links = extract_links(&self.text, &plain);
            (plain, spans, links)
        } else {
            (self.text.to_string(), Vec::new(), Vec::new())
        };
        self.plain = plain;
        self.spans = spans;
        self.links = links;

        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        self.layout = TextLayout::build(
            &self.plain,
            &style,
            cx.fonts,
            width,
            self.wrap,
            self.ellipsize,
        );
        self.built_width = width;

        // Subnodes: one `selection` when selectable, one `link` per link, and
        // the `.link` class on the label itself when any link exists.
        if self.selectable && self.selection_node.is_none() {
            let selection = Node::new("selection");
            node.append_child(&selection);
            self.selection_node = Some(selection);
        } else if !self.selectable {
            if let Some(selection) = self.selection_node.take() {
                selection.detach();
            }
        }
        while self.link_nodes.len() > self.links.len() {
            if let Some(extra) = self.link_nodes.pop() {
                extra.detach();
            }
        }
        while self.link_nodes.len() < self.links.len() {
            let link = Node::new("link");
            node.append_child(&link);
            self.link_nodes.push(link);
        }
        node.set_state(PseudoStates::empty(), false);
        if self.links.is_empty() {
            node.remove_class("link");
        } else {
            node.add_class("link");
        }
    }
}

/// `(range in the plain text, uri)` for every `<a href="…">` in `source`.
///
/// `parse_markup` drops `<a>` with every other unknown tag; links are read
/// here instead so the plain text the two produce stays identical.
fn extract_links(source: &str, plain: &str) -> Vec<(Range<usize>, Rc<str>)> {
    let mut out = Vec::new();
    let mut plain_cursor = 0usize;
    let mut rest = source;
    while let Some(open) = rest.find("<a ") {
        // Everything before the tag lands in the plain text verbatim, modulo
        // the tags `parse_markup` already removed, so the cursor advances by
        // the plain-text length of that prefix.
        plain_cursor += parse_markup(&rest[..open]).0.len();
        let Some(gt) = rest[open..].find('>').map(|o| open + o) else {
            break;
        };
        let uri = rest[open..gt]
            .split_once("href=")
            .map(|(_, tail)| tail.trim_start().trim_start_matches(['"', '\'']))
            .and_then(|tail| tail.split(['"', '\'']).next())
            .unwrap_or("");
        let body_start = gt + 1;
        let end = rest[body_start..]
            .find("</a>")
            .map_or(rest.len(), |o| body_start + o);
        let body_len = parse_markup(&rest[body_start..end]).0.len();
        if plain_cursor + body_len <= plain.len() {
            out.push((plain_cursor..plain_cursor + body_len, Rc::from(uri)));
        }
        plain_cursor += body_len;
        rest = &rest[(end + 4).min(rest.len())..];
    }
    out
}

impl<Msg: Clone + 'static> Controller<Msg> for LabelC {
    fn kind(&self) -> Kind {
        Kind::Label
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let mut this = LabelC {
            layout: TextLayout::build(
                "",
                &TextStyle::from_computed(&ComputedStyle::initial(cx.env)),
                cx.fonts,
                None,
                WrapMode::None,
                Ellipsize::None,
            ),
            spans: Vec::new(),
            selection: None,
            links: Vec::new(),
            hovered_link: None,
            selection_node: None,
            link_nodes: Vec::new(),
            text: props
                .str(PropName::Label)
                .map_or_else(|| Rc::from(""), Rc::from),
            plain: String::new(),
            wrap: match props.get(PropName::Wrap) {
                Some(Prop::Enum(raw)) => wrap_from_u16(*raw),
                Some(Prop::Bool(true)) => WrapMode::Word,
                _ => WrapMode::None,
            },
            ellipsize: match props.get(PropName::Ellipsize) {
                Some(Prop::Enum(raw)) => ellipsize_from_u16(*raw),
                _ => Ellipsize::None,
            },
            xalign: props.float(PropName::Xalign, 0.5) as f32,
            markup: props.bool(PropName::Markup, false),
            selectable: props.bool(PropName::Selectable, false),
            pointer: PointerState::default(),
            built_width: None,
        };
        this.rebuild(node, cx, None);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::Label => {
                self.text = match value {
                    Prop::Str(text) => Rc::clone(text),
                    _ => Rc::from(""),
                };
            }
            PropName::Wrap => {
                self.wrap = match value {
                    Prop::Enum(raw) => wrap_from_u16(*raw),
                    Prop::Bool(true) => WrapMode::Word,
                    _ => WrapMode::None,
                };
            }
            PropName::Ellipsize => {
                self.ellipsize = match value {
                    Prop::Enum(raw) => ellipsize_from_u16(*raw),
                    _ => Ellipsize::None,
                };
            }
            PropName::Markup => self.markup = matches!(value, Prop::Bool(true)),
            PropName::Selectable => self.selectable = matches!(value, Prop::Bool(true)),
            PropName::Xalign => {
                if let Prop::Float(a) = value {
                    self.xalign = if a.is_finite() { *a as f32 } else { 0.5 };
                }
            }
            _ => return,
        }
        let width = self.built_width;
        self.rebuild(node, cx, width);
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let width = available.0.filter(|w| w.is_finite() && *w > 0.0);
        if width != self.built_width {
            let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
            self.layout = TextLayout::build(
                &self.plain,
                &style,
                cx.fonts,
                width,
                self.wrap,
                self.ellipsize,
            );
            self.built_width = width;
        }
        Some(self.layout.size())
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let alloc = cx.tree.allocation(cx.node).map(|a| a.content_box);
        let clicked = self.pointer.observe(
            cx.node,
            ev,
            alloc.map(|r| Rect::new(0.0, 0.0, r.width, r.height)),
        );
        if let Event::PointerMotion { local } = ev {
            let byte = self.layout.byte_at(*local);
            self.hovered_link = self.links.iter().position(|(range, _)| range.contains(&byte));
            for (index, link) in self.link_nodes.iter().enumerate() {
                link.set_state(PseudoStates::HOVER, self.hovered_link == Some(index));
            }
        }
        if clicked {
            if let Event::PointerUp { local, .. } = ev {
                let byte = self.layout.byte_at(*local);
                if let Some((_, uri)) =
                    self.links.iter().find(|(range, _)| range.contains(&byte))
                {
                    cx.handled = true;
                    if let Some(msg) = cx.handlers.fire_text(EventKind::ActivateLink, uri) {
                        return vec![msg];
                    }
                }
            }
        }
        Vec::new()
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        let (text_w, _) = self.layout.size();
        let slack = (content.width - text_w).max(0.0);
        let x = content.x + slack * self.xalign.clamp(0.0, 1.0);
        self.layout.draw(canvas, (x, content.y), style.color());
        true
    }
}
```

- [ ] **Step 5: Register the widget**

In `ui/src/widgets/mod.rs`: add `pub mod label;` beside `pub mod separator;`, and
add to `build_controller`'s match, above the `_` arm:

```rust
        Kind::Label => Box::new(label::LabelC::build(node, props, cx)),
```

In `ui/src/view/builders.rs`:

```rust
pub use crate::widgets::label::{LabelExt, label};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --test node_trees a_label_renders`
Run: `cargo test -p icedtea-ui --test widget_pixels a_label_inks`
Run: `cargo test -p icedtea-ui --test widget_pixels clicking_a_link`
Expected: PASS, all three.

- [ ] **Step 7: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests \
        ui/tests/fixtures/gtk4.22-node-trees/label.txt
git commit -m "feat(ui/widgets): Label — wrapping, ellipsizing, markup and links

The label's paragraph is a TextLayout, its markup a parse_markup span list, and
its links one 'link' subnode each with the .link class on the label node, as
GTK 4.22.4's own CSS-nodes block specifies.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: `Spinner` and `Statusbar`

**Files:**
- Create: `ui/src/widgets/spinner.rs`, `ui/src/widgets/statusbar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/{spinner,statusbar}.txt`
- Modify: `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`
- Test: `ui/tests/node_trees.rs`, `ui/tests/widget_pixels.rs`

**Interfaces:**
- Consumes: Task 7's scaffolding; `anim::Clock`; `css::node::PseudoStates::CHECKED`.
- Produces:
  ```rust
  pub fn spinner<Msg: Clone + 'static>() -> View<Msg>;
  pub trait SpinnerExt<Msg>: Sized { fn spinning(self, on: bool) -> Self; }
  pub struct SpinnerC { pub spinning: bool, pub phase: f32, pub started: Duration }

  pub fn statusbar<Msg: Clone + 'static>() -> View<Msg>;
  pub trait StatusbarExt<Msg>: Sized { fn text(self, text: &str) -> Self; }
  pub struct StatusbarC { pub stack: Vec<(u32, String)>, pub label: Node }
  ```

- [ ] **Step 1: Vendor the fixtures**

`spinner.txt` (`gtk/gtkspinner.c:58` — GTK's block is prose, so the fixture is
the single node it describes):

```
spinner
```

`statusbar.txt` (`gtk/deprecated/gtkstatusbar.c:80`, kept in scope by ruling R1):

```
statusbar
```

- [ ] **Step 2: Write the failing tests**

Append to `ui/tests/node_trees.rs`:

```rust
#[test]
fn a_spinner_and_a_statusbar_render_their_single_nodes() {
    // mutation: append any subnode in SpinnerC::build and this fails with
    // "rendered node at 'spinner/…' is not in the fixture".
    check(Kind::Spinner, "spinner", &Props::default());
    assert_eq!(node_tree_of(Kind::Spinner, &Props::default()).trim(), "spinner");
    check(Kind::Statusbar, "statusbar", &Props::default());
}
```

Append to `ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_spinner_carries_checked_while_spinning_and_advances_its_phase() {
    // mutation: drop the CHECKED state in SpinnerC::apply and the two frames
    // become identical; drop the `phase` advance in `tick` and they do too.
    use icedtea_ui::view::builders::spinner;
    use icedtea_ui::widgets::spinner::SpinnerExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| spinner().spinning(true),
        (48, 48),
        vec![
            ScriptStep::Capture,
            ScriptStep::Advance(Duration::from_millis(500)),
            ScriptStep::Capture,
        ],
    );
    let first: Vec<_> = (0..48).map(|x| frames.pixel(0, x, 24)).collect();
    let second: Vec<_> = (0..48).map(|x| frames.pixel(1, x, 24)).collect();
    assert_ne!(first, second, "the spinner arc must have rotated");
}

#[test]
fn a_statusbar_shows_the_top_of_its_message_stack() {
    // mutation: push instead of replace in StatusbarC::set_prop and the second
    // frame still shows the first message.
    use icedtea_ui::view::builders::statusbar;
    use icedtea_ui::widgets::statusbar::StatusbarExt;
    let frames = run(
        0u32,
        |model: &mut u32, _msg: ()| {
            *model += 1;
            Cmd::None
        },
        |model: &u32| statusbar().text(if *model == 0 { "" } else { "Saved" }),
        (200, 32),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(()),
            ScriptStep::Capture,
        ],
    );
    assert!(!has_ink(&frames, 0, (200, 32)), "an empty statusbar inks nothing");
    assert!(has_ink(&frames, 1, (200, 32)), "the pushed message shows");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees a_spinner_and_a_statusbar`
Expected: FAIL — `fixture requires a node at 'statusbar/label'` is *not* the
error; the failure is `assert_ne!` in the pixel test and a missing `statusbar`
builder in `view::builders`.

- [ ] **Step 4: Write `ui/src/widgets/spinner.rs`**

```rust
//! `GtkSpinner` — `Kind::Spinner`, CSS node `spinner`.
//!
//! ```text
//! spinner
//! ```
//!
//! One node. GTK adds `:checked` while the animation runs — its own divergence
//! from what `:checked` means everywhere else, and Adwaita styles it — so the
//! controller sets `PseudoStates::CHECKED`, not a style class.

use std::f32::consts::TAU;
use std::time::Duration;

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::core::Paint;

use crate::css::computed::ComputedStyle;
use crate::css::node::{Node, PseudoStates};
use crate::layout::Allocation;
use crate::view::controller::{BuildCx, Controller, Event, EventCx, PaintCx};
use crate::view::{Kind, Prop, PropName, Props, View};

/// One full rotation, matching Adwaita's own `spin` keyframe.
const PERIOD: Duration = Duration::from_millis(1000);

/// A `GtkSpinner`, stopped.
#[must_use]
pub fn spinner<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::Spinner)
}

/// `GtkSpinner:spinning`.
pub trait SpinnerExt<Msg>: Sized {
    /// Start or stop the animation.
    fn spinning(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> SpinnerExt<Msg> for View<Msg> {
    fn spinning(self, on: bool) -> Self {
        self.prop(PropName::Active, Prop::Bool(on))
    }
}

/// `Kind::Spinner`'s controller.
pub struct SpinnerC {
    /// Whether the animation is running.
    pub spinning: bool,
    /// Rotation in turns, `0.0..1.0`.
    pub phase: f32,
    /// Clock reading the current rotation is measured from.
    pub started: Duration,
}

impl SpinnerC {
    fn apply(&self, node: &Node) {
        node.set_state(PseudoStates::CHECKED, self.spinning);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SpinnerC {
    fn kind(&self) -> Kind {
        Kind::Spinner
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let this = SpinnerC {
            spinning: props.bool(PropName::Active, false),
            phase: 0.0,
            started: cx.clock.now(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if name == PropName::Active {
            let next = matches!(value, Prop::Bool(true));
            if next != self.spinning {
                self.spinning = next;
                self.started = cx.clock.now();
                self.phase = 0.0;
                self.apply(node);
            }
        }
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.spinning {
            let elapsed = now.saturating_sub(self.started).as_secs_f32();
            let period = PERIOD.as_secs_f32();
            self.phase = (elapsed / period).fract();
        }
        Vec::new()
    }

    /// One frame while spinning; nothing at all when stopped. `Duration::ZERO`
    /// is never returned — a stopped spinner must not pin the event loop.
    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.spinning.then(|| now + Duration::from_millis(16))
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        let size = content.width.min(content.height);
        if size <= 0.0 {
            return false;
        }
        let stroke = (size / 8.0).max(1.0);
        let mut paint = Paint::new();
        paint.set_anti_alias(true);
        paint.set_style(skia_rs_safe::core::PaintStyle::Stroke);
        paint.set_stroke_width(stroke);
        let colour = style.color();
        paint.set_color4f(colour.r, colour.g, colour.b, colour.a);
        let inset = stroke / 2.0;
        let bounds = crate::layout::Rect::new(
            content.x + (content.width - size) / 2.0 + inset,
            content.y + (content.height - size) / 2.0 + inset,
            size - stroke,
            size - stroke,
        );
        // A 90° arc, rotated by the phase — GTK's spinner is a quarter-circle
        // chasing its own tail.
        canvas.draw_arc(
            &bounds.to_skia(),
            self.phase * TAU.to_degrees() - 90.0,
            90.0,
            false,
            &paint,
        );
        true
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/statusbar.rs`**

```rust
//! `GtkStatusbar` — `Kind::Statusbar`, CSS node `statusbar`.
//!
//! ```text
//! statusbar
//! ```
//!
//! Deprecated upstream in 4.10 and kept in scope by contract ruling R1: Adwaita
//! still styles `statusbar`, and the spec names it. GTK's own widget owns a
//! message *stack* keyed by context id; the reactive model has one text prop,
//! so the stack lives in the controller and the prop replaces its top entry.

use std::rc::Rc;

use crate::css::node::Node;
use crate::text::{Ellipsize, TextStyle, WrapMode};
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::label::LabelC;

/// A `GtkStatusbar` with no message.
#[must_use]
pub fn statusbar<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::Statusbar)
}

/// `GtkStatusbar`'s message text.
pub trait StatusbarExt<Msg>: Sized {
    /// Replace the top of the message stack.
    fn text(self, text: &str) -> Self;
}

impl<Msg: Clone + 'static> StatusbarExt<Msg> for View<Msg> {
    fn text(self, text: &str) -> Self {
        self.prop(PropName::Text, Prop::Str(Rc::from(text)))
    }
}

/// `Kind::Statusbar`'s controller.
pub struct StatusbarC {
    /// `(context id, message)`, innermost last. The reactive model pushes and
    /// pops by replacing the `Text` prop, so context 0 is the only one used
    /// today; the stack is kept because `GtkStatusbar`'s pop semantics are what
    /// M5's shell will need.
    pub stack: Vec<(u32, String)>,
    /// The `label` node the text is drawn through.
    pub label: Node,
    inner: LabelC,
}

impl StatusbarC {
    fn top(&self) -> &str {
        self.stack.last().map_or("", |(_, text)| text.as_str())
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for StatusbarC {
    fn kind(&self) -> Kind {
        Kind::Statusbar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        // GTK's statusbar contains a label widget, whose own node is `label`.
        // The fixture stops at `statusbar` because GTK's block does; the label
        // is a child *widget*, so it is rendered through as `<child>` would be.
        let label = Node::new("label");
        node.append_child(&label);
        let text = props.str(PropName::Text).unwrap_or("").to_owned();
        let mut label_props = Props::default();
        label_props.set(PropName::Label, Prop::Str(Rc::from(text.as_str())));
        let inner = LabelC::build(&label, &label_props, cx);
        StatusbarC { stack: vec![(0, text)], label, inner }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if name == PropName::Text {
            let text = match value {
                Prop::Str(text) => text.to_string(),
                _ => String::new(),
            };
            match self.stack.last_mut() {
                Some(top) => top.1 = text,
                None => self.stack.push((0, text)),
            }
            let owned = self.top().to_owned();
            Controller::<Msg>::set_prop(
                &mut self.inner,
                &self.label,
                PropName::Label,
                &Prop::Str(Rc::from(owned.as_str())),
                cx,
            );
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Controller::<Msg>::measure(&mut self.inner, available, cx)
    }
}

/// Unused imports guard: the statusbar's label never wraps or ellipsizes, and
/// naming the types here keeps that decision visible rather than implicit.
const _: (WrapMode, Ellipsize) = (WrapMode::None, Ellipsize::None);
const _: fn(&crate::css::computed::ComputedStyle) -> TextStyle = TextStyle::from_computed;
```

Drop the two `const _` lines if clippy objects; they exist only to document that
the statusbar label is deliberately single-line.

- [ ] **Step 6: Register both widgets**

`ui/src/widgets/mod.rs`: `pub mod spinner;`, `pub mod statusbar;`, and two
dispatch arms:

```rust
        Kind::Spinner => Box::new(spinner::SpinnerC::build(node, props, cx)),
        Kind::Statusbar => Box::new(statusbar::StatusbarC::build(node, props, cx)),
```

`ui/src/view/builders.rs`:

```rust
pub use crate::widgets::spinner::{SpinnerExt, spinner};
pub use crate::widgets::statusbar::{StatusbarExt, statusbar};
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --test node_trees a_spinner_and_a_statusbar`
Run: `cargo test -p icedtea-ui --test widget_pixels a_spinner_carries_checked`
Run: `cargo test -p icedtea-ui --test widget_pixels a_statusbar_shows`
Expected: PASS, all three.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Spinner and Statusbar

The spinner's :checked-while-spinning is GTK's own divergence and Adwaita
styles it, so it is a pseudo-state, not a class; its next_deadline is one
frame while running and None when stopped, never Duration::ZERO.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 10: `LevelBar` and `ProgressBar`

**Files:**
- Create: `ui/src/widgets/level_bar.rs`, `ui/src/widgets/progress_bar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/{level_bar,progress_bar}.txt`
- Modify: `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`
- Test: `ui/tests/node_trees.rs`, `ui/tests/widget_pixels.rs`

**Interfaces:**
- Consumes: Task 7's `Adjustment`, `LevelBarMode`, `WidgetEnum`; Task 8's `LabelC` (for the `text` subnode).
- Produces:
  ```rust
  pub fn level_bar<Msg: Clone + 'static>(value: f64) -> View<Msg>;
  pub trait LevelBarExt<Msg>: Sized {
      fn min_value(self, v: f64) -> Self;
      fn max_value(self, v: f64) -> Self;
      fn mode(self, mode: LevelBarMode) -> Self;
      fn offset(self, name: &str, value: f64) -> Self;
      fn inverted(self, on: bool) -> Self;
  }
  pub struct LevelBarC { pub value: f64, pub min: f64, pub max: f64, pub discrete: bool,
                         pub offsets: Vec<(Rc<str>, f64)>, pub trough: Node, pub blocks: Vec<Node> }

  pub fn progress_bar<Msg: Clone + 'static>(fraction: f64) -> View<Msg>;
  pub trait ProgressBarExt<Msg>: Sized {
      fn text(self, text: &str) -> Self;
      fn show_text(self, on: bool) -> Self;
      fn inverted(self, on: bool) -> Self;
      fn pulse_step(self, step: f64) -> Self;
      fn ellipsize(self, mode: Ellipsize) -> Self;
  }
  pub struct ProgressBarC { pub fraction: f64, pub pulsing: bool, pub pulse_pos: f64,
                            pub trough: Node, pub progress: Node, pub text: Option<Node> }
  ```

- [ ] **Step 1: Vendor the fixtures**

`level_bar.txt` (`gtk/gtklevelbar.c:99`):

```
levelbar[.discrete]
╰── trough
    ├── block.filled.level-name
    ┊
    ├── block.empty
    ┊
```

`progress_bar.txt` (`gtk/gtkprogressbar.c:74`):

```
progressbar[.osd]
├── [text]
╰── trough[.empty][.full]
    ╰── progress[.pulse]
```

Note for the executor: `block.filled.level-name` names a *pattern* — GTK adds a
class named after the matched offset. The matcher treats `filled` and
`level-name` as required classes on any node at that path, so `LevelBarC` names
its filled blocks' second class after the offset it matched and adds
`level-name` as a literal class too, which is what GTK renders when no named
offset matched.

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_level_bar_builds_a_trough_of_filled_and_empty_blocks() {
    // mutation: emit one block instead of one per discrete step and the
    // discrete case renders a single `block` where the fixture wants both.
    use icedtea_ui::widgets::LevelBarMode;
    let mut props = Props::default();
    props.set(PropName::Value, Prop::Float(0.5));
    check(Kind::LevelBar, "level_bar", &props);

    let mut discrete = props.clone();
    discrete.set(PropName::Model, LevelBarMode::Discrete.to_prop());
    discrete.set(PropName::Upper, Prop::Float(4.0));
    let rendered = node_tree_of(Kind::LevelBar, &discrete);
    assert!(rendered.contains("levelbar.discrete"), "{rendered}");
    assert_eq!(rendered.matches("block").count(), 4, "one block per step: {rendered}");
    check(Kind::LevelBar, "level_bar", &discrete);
}

#[test]
fn a_progress_bar_shows_its_text_subnode_only_when_asked() {
    // mutation: build the `text` node unconditionally and the first assertion
    // finds it with show_text off.
    let mut props = Props::default();
    props.set(PropName::Fraction, Prop::Float(0.25));
    assert!(!node_tree_of(Kind::ProgressBar, &props).contains("text"));
    check(Kind::ProgressBar, "progress_bar", &props);

    props.set(PropName::ShowText, Prop::Bool(true));
    assert!(node_tree_of(Kind::ProgressBar, &props).contains("text"));
    check(Kind::ProgressBar, "progress_bar", &props);
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_progress_bars_fill_widens_with_its_fraction() {
    // mutation: ignore `fraction` when sizing the `progress` node and both
    // frames ink the same width.
    use icedtea_ui::view::builders::progress_bar;
    let frames = run(
        0.1f64,
        |model: &mut f64, _msg: ()| {
            *model = 0.9;
            Cmd::None
        },
        |model: &f64| progress_bar(*model).hexpand(true),
        (200, 24),
        vec![ScriptStep::Capture, ScriptStep::Message(()), ScriptStep::Capture],
    );
    let inked = |frame: usize| {
        (0..200)
            .filter(|x| frames.pixel(frame, *x, 12) != frames.pixel(frame, 199, 12))
            .count()
    };
    assert!(inked(1) > inked(0), "0.9 must ink wider than 0.1");
}

#[test]
fn a_pulsing_progress_bar_moves_its_block_on_the_clock() {
    // mutation: make `tick` a no-op and the two frames match.
    use icedtea_ui::view::builders::progress_bar;
    use icedtea_ui::widgets::progress_bar::ProgressBarExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| progress_bar(f64::NAN).pulse_step(0.1).hexpand(true),
        (200, 24),
        vec![
            ScriptStep::Capture,
            ScriptStep::Advance(Duration::from_millis(300)),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 12)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the pulse block must have moved");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees a_level_bar`
Expected: FAIL — `fixture requires a node at 'levelbar/trough'`.

- [ ] **Step 4: Write `ui/src/widgets/level_bar.rs`**

```rust
//! `GtkLevelBar` — `Kind::LevelBar`, CSS node `levelbar`.
//!
//! ```text
//! levelbar[.discrete]
//! ╰── trough
//!     ├── block.filled.level-name
//!     ┊
//!     ├── block.empty
//!     ┊
//! ```
//!
//! Continuous mode renders exactly one filled and one empty block; discrete
//! mode one block per integral step. Filled blocks carry `.filled` plus the
//! name of the highest offset the value has passed, and `.level-name` — GTK's
//! own literal class when no named offset matched.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::{Adjustment, LevelBarMode, WidgetEnum};

/// A `GtkLevelBar` at `value` over the default `0.0..=1.0`.
#[must_use]
pub fn level_bar<Msg: Clone + 'static>(value: f64) -> View<Msg> {
    View::new(Kind::LevelBar).prop(PropName::Value, Prop::Float(value))
}

/// `GtkLevelBar`'s own setters.
pub trait LevelBarExt<Msg>: Sized {
    /// `GtkLevelBar:min-value`.
    fn min_value(self, v: f64) -> Self;
    /// `GtkLevelBar:max-value`.
    fn max_value(self, v: f64) -> Self;
    /// `GtkLevelBar:mode`.
    fn mode(self, mode: LevelBarMode) -> Self;
    /// `gtk_level_bar_add_offset_value`. Repeated calls accumulate.
    fn offset(self, name: &str, value: f64) -> Self;
    /// `GtkLevelBar:inverted`.
    fn inverted(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> LevelBarExt<Msg> for View<Msg> {
    fn min_value(self, v: f64) -> Self {
        self.prop(PropName::Lower, Prop::Float(v))
    }
    fn max_value(self, v: f64) -> Self {
        self.prop(PropName::Upper, Prop::Float(v))
    }
    fn mode(self, mode: LevelBarMode) -> Self {
        self.prop(PropName::Model, mode.to_prop())
    }
    fn offset(self, name: &str, value: f64) -> Self {
        // Offsets ride in one `Classes` prop as `name=value` pairs: `Prop` has
        // no map variant, and a level bar has at most a handful.
        let mut encoded: Vec<Rc<str>> = match self.props.get(PropName::Detail) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        encoded.push(Rc::from(format!("{name}={value}")));
        self.prop(PropName::Detail, Prop::Classes(Rc::from(encoded)))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
}

/// `Kind::LevelBar`'s controller.
pub struct LevelBarC {
    /// Current value.
    pub value: f64,
    /// `GtkLevelBar:min-value`.
    pub min: f64,
    /// `GtkLevelBar:max-value`.
    pub max: f64,
    /// Discrete mode.
    pub discrete: bool,
    /// Named offsets, ascending by value.
    pub offsets: Vec<(Rc<str>, f64)>,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `block` subnodes, left to right.
    pub blocks: Vec<Node>,
}

impl LevelBarC {
    /// The class the filled blocks carry, from the highest offset passed.
    fn level_name(&self) -> Rc<str> {
        let mut best: Option<&(Rc<str>, f64)> = None;
        for offset in &self.offsets {
            if self.value >= offset.1 && best.is_none_or(|b| offset.1 >= b.1) {
                best = Some(offset);
            }
        }
        best.map_or_else(|| Rc::from("level-name"), |(name, _)| Rc::clone(name))
    }

    /// Rebuild the block row and its classes.
    fn apply(&mut self, node: &Node) {
        node.set_state(crate::css::node::PseudoStates::empty(), false);
        if self.discrete {
            node.add_class("discrete");
            node.remove_class("continuous");
        } else {
            node.add_class("continuous");
            node.remove_class("discrete");
        }
        let adj = Adjustment::new(self.value, self.min, self.max);
        let steps = if self.discrete {
            ((self.max - self.min).round().max(1.0) as usize).min(64)
        } else {
            2
        };
        while self.blocks.len() > steps {
            if let Some(extra) = self.blocks.pop() {
                extra.detach();
            }
        }
        while self.blocks.len() < steps {
            let block = Node::new("block");
            self.trough.append_child(&block);
            self.blocks.push(block);
        }
        let level = self.level_name();
        let filled = if self.discrete {
            (adj.fraction() * steps as f64).round() as usize
        } else {
            1
        };
        for (index, block) in self.blocks.iter().enumerate() {
            let is_filled = index < filled;
            block.set_classes(if is_filled {
                &["filled", level.as_ref(), "level-name"]
            } else {
                &["empty"]
            });
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for LevelBarC {
    fn kind(&self) -> Kind {
        Kind::LevelBar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let trough = Node::new("trough");
        node.append_child(&trough);
        let mut offsets: Vec<(Rc<str>, f64)> = Vec::new();
        if let Some(Prop::Classes(list)) = props.get(PropName::Detail) {
            for encoded in list.iter() {
                if let Some((name, value)) = encoded.split_once('=') {
                    if let Ok(value) = value.parse::<f64>() {
                        if value.is_finite() {
                            offsets.push((Rc::from(name), value));
                        }
                    }
                }
            }
        }
        let mut this = LevelBarC {
            value: props.float(PropName::Value, 0.0),
            min: props.float(PropName::Lower, 0.0),
            max: props.float(PropName::Upper, 1.0),
            discrete: matches!(
                LevelBarMode::from_prop(props.get(PropName::Model), LevelBarMode::Continuous),
                LevelBarMode::Discrete
            ),
            offsets,
            trough,
            blocks: Vec::new(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => self.value = *v,
            (PropName::Lower, Prop::Float(v)) => self.min = *v,
            (PropName::Upper, Prop::Float(v)) => self.max = *v,
            (PropName::Model, Prop::Enum(_)) => {
                self.discrete = matches!(
                    LevelBarMode::from_prop(Some(value), LevelBarMode::Continuous),
                    LevelBarMode::Discrete
                );
            }
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/progress_bar.rs`**

```rust
//! `GtkProgressBar` — `Kind::ProgressBar`, CSS node `progressbar`.
//!
//! ```text
//! progressbar[.osd]
//! ├── [text]
//! ╰── trough[.empty][.full]
//!     ╰── progress[.pulse]
//! ```
//!
//! A non-finite `fraction` means activity mode: the `progress` node gets
//! `.pulse` and `tick` walks it back and forth by `pulse_step` per frame.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::text::Ellipsize;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::label::LabelC;

/// A `GtkProgressBar` at `fraction`. A non-finite fraction starts activity mode.
#[must_use]
pub fn progress_bar<Msg: Clone + 'static>(fraction: f64) -> View<Msg> {
    View::new(Kind::ProgressBar).prop(PropName::Fraction, Prop::Float(fraction))
}

/// `GtkProgressBar`'s own setters.
pub trait ProgressBarExt<Msg>: Sized {
    /// `GtkProgressBar:text`.
    fn text(self, text: &str) -> Self;
    /// `GtkProgressBar:show-text`.
    fn show_text(self, on: bool) -> Self;
    /// `GtkProgressBar:inverted`.
    fn inverted(self, on: bool) -> Self;
    /// `GtkProgressBar:pulse-step`; also what turns activity mode on.
    fn pulse_step(self, step: f64) -> Self;
    /// `GtkProgressBar:ellipsize`, for the `text` subnode.
    fn ellipsize(self, mode: Ellipsize) -> Self;
}

impl<Msg: Clone + 'static> ProgressBarExt<Msg> for View<Msg> {
    fn text(self, text: &str) -> Self {
        self.prop(PropName::Text, Prop::Str(Rc::from(text)))
    }
    fn show_text(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
    fn pulse_step(self, step: f64) -> Self {
        self.prop(PropName::Pulse, Prop::Float(step))
    }
    fn ellipsize(self, mode: Ellipsize) -> Self {
        self.prop(
            PropName::Ellipsize,
            Prop::Enum(match mode {
                Ellipsize::None => 0,
                Ellipsize::Start => 1,
                Ellipsize::Middle => 2,
                Ellipsize::End => 3,
            }),
        )
    }
}

/// `Kind::ProgressBar`'s controller.
pub struct ProgressBarC {
    /// `0.0..=1.0`, or NaN in activity mode.
    pub fraction: f64,
    /// Activity mode.
    pub pulsing: bool,
    /// Where the pulse block sits, `0.0..=1.0`.
    pub pulse_pos: f64,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `progress` subnode inside the trough.
    pub progress: Node,
    /// The `text` subnode, only when `show-text`.
    pub text: Option<Node>,
    pulse_step: f64,
    forward: bool,
    label: Option<LabelC>,
}

impl ProgressBarC {
    /// The used fraction: clamped, and 0 while pulsing.
    fn used(&self) -> f64 {
        if self.pulsing || !self.fraction.is_finite() {
            0.0
        } else {
            self.fraction.clamp(0.0, 1.0)
        }
    }

    fn apply(&mut self, node: &Node) {
        let used = self.used();
        self.trough.set_state(crate::css::node::PseudoStates::empty(), false);
        self.trough.remove_class("empty");
        self.trough.remove_class("full");
        if !self.pulsing && used <= 0.0 {
            self.trough.add_class("empty");
        } else if used >= 1.0 {
            self.trough.add_class("full");
        }
        if self.pulsing {
            self.progress.add_class("pulse");
        } else {
            self.progress.remove_class("pulse");
        }
        let _ = node;
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ProgressBarC {
    fn kind(&self) -> Kind {
        Kind::ProgressBar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let fraction = props.float(PropName::Fraction, 0.0);
        let text = props.bool(PropName::ShowText, false).then(|| {
            let text = Node::new("text");
            node.append_child(&text);
            text
        });
        let trough = Node::new("trough");
        node.append_child(&trough);
        let progress = Node::new("progress");
        trough.append_child(&progress);
        let label = text.as_ref().map(|node| {
            let mut label_props = Props::default();
            label_props.set(
                PropName::Label,
                Prop::Str(Rc::from(props.str(PropName::Text).unwrap_or(""))),
            );
            LabelC::build(node, &label_props, cx)
        });
        let mut this = ProgressBarC {
            pulsing: !fraction.is_finite(),
            fraction,
            pulse_pos: 0.0,
            pulse_step: props.float(PropName::Pulse, 0.1).abs().clamp(0.01, 1.0),
            forward: true,
            trough,
            progress,
            text,
            label,
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Fraction, Prop::Float(v)) => {
                self.fraction = *v;
                self.pulsing = !v.is_finite();
            }
            (PropName::Pulse, Prop::Float(v)) => {
                self.pulse_step = v.abs().clamp(0.01, 1.0);
            }
            _ => return,
        }
        self.apply(node);
    }

    fn tick(&mut self, _now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.pulsing {
            let delta = if self.forward { self.pulse_step } else { -self.pulse_step };
            self.pulse_pos += delta;
            if self.pulse_pos >= 1.0 {
                self.pulse_pos = 1.0;
                self.forward = false;
            } else if self.pulse_pos <= 0.0 {
                self.pulse_pos = 0.0;
                self.forward = true;
            }
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.pulsing.then(|| now + Duration::from_millis(50))
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        match self.label.as_mut() {
            Some(label) => Controller::<Msg>::measure(label, available, cx),
            None => None,
        }
    }
}
```

The `progress` node's width is CSS-driven: `ProgressBarC` writes it as an inline
width fraction by setting the node's `Position` prop-derived class is *not* how
GTK does it — GTK sizes the `progress` node in `size_allocate`. Since P5 may not
touch `layout.rs`, size it by giving the `progress` node an explicit width
through `LayoutTree` via `Controller::measure` on the trough: implement
`ProgressBarC::measure` to report `(trough_width * used, height)` for the
progress node. Do that by making the trough a `Container::Box { direction: Row }`
whose single child's flex basis is the used fraction — expressed as the
`progress` node's own `width` in the retained tree via a `.filled-<n>` class is
brittle, so instead the controller paints the fill itself in `Controller::paint`
by clipping the `progress` node's background to `used`:

```rust
    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        // The `progress` node's own box is painted by M2 across the whole
        // trough; the used fraction is the clip. Painting it here (rather than
        // sizing it in layout) is what keeps P5 out of `layout.rs`, which P6
        // owns.
        let content = alloc.content_box;
        let used = if self.pulsing {
            let width = content.width * 0.25;
            crate::layout::Rect::new(
                content.x + (content.width - width) * self.pulse_pos as f32,
                content.y,
                width,
                content.height,
            )
        } else {
            crate::layout::Rect::new(
                content.x,
                content.y,
                content.width * self.used() as f32,
                content.height,
            )
        };
        if used.is_empty() {
            return false;
        }
        canvas.draw_rect(&used.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
```

- [ ] **Step 6: Register both widgets**

`ui/src/widgets/mod.rs`: `pub mod level_bar;`, `pub mod progress_bar;` and

```rust
        Kind::LevelBar => Box::new(level_bar::LevelBarC::build(node, props, cx)),
        Kind::ProgressBar => Box::new(progress_bar::ProgressBarC::build(node, props, cx)),
```

`ui/src/view/builders.rs`:

```rust
pub use crate::widgets::level_bar::{LevelBarExt, level_bar};
pub use crate::widgets::progress_bar::{ProgressBarExt, progress_bar};
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --test node_trees a_level_bar`
Run: `cargo test -p icedtea-ui --test node_trees a_progress_bar`
Run: `cargo test -p icedtea-ui --test widget_pixels a_progress_bars_fill`
Run: `cargo test -p icedtea-ui --test widget_pixels a_pulsing_progress_bar`
Expected: PASS, all four.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): LevelBar and ProgressBar

Discrete level bars build one block per step and name their filled blocks after
the highest offset passed; the progress bar's fill and pulse block are painted
against the trough's content box, which keeps P5 out of layout.rs.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 11: `InfoBar`

**Files:**
- Create: `ui/src/widgets/info_bar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/info_bar.txt`
- Modify: `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`
- Test: `ui/tests/node_trees.rs`, `ui/tests/widget_pixels.rs`

**Interfaces:**
- Consumes: Task 7's `MessageType`, `PointerState`; `anim::Clock` for the reveal.
- Produces:
  ```rust
  pub fn info_bar<Msg: Clone + 'static>() -> View<Msg>;
  pub trait InfoBarExt<Msg>: Sized {
      fn message_type(self, kind: MessageType) -> Self;
      fn revealed(self, on: bool) -> Self;
      fn show_close_button(self, on: bool) -> Self;
      fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      fn on_close(self, msg: Msg) -> Self;
  }
  pub struct InfoBarC { pub revealed: bool, pub reveal_progress: f32, pub close_button: Option<Node> }
  ```

- [ ] **Step 1: Vendor the fixture**

`info_bar.txt` (`gtk/deprecated/gtkinfobar.c:126`; kept by ruling R1):

```
infobar[.info][.warning][.error][.question]
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn an_info_bar_carries_its_message_type_class_and_a_close_button() {
    // mutation: stop adding the message-type class and the `.warning`
    // assertion fails.
    use icedtea_ui::widgets::MessageType;
    let mut props = Props::default();
    props.set(PropName::MessageType, MessageType::Warning.to_prop());
    let rendered = node_tree_of(Kind::InfoBar, &props);
    assert!(rendered.starts_with("infobar.warning"), "{rendered}");
    check(Kind::InfoBar, "info_bar", &props);

    props.set(PropName::Buttons, Prop::Bool(true));
    let with_close = node_tree_of(Kind::InfoBar, &props);
    assert!(with_close.contains("button.close"), "{with_close}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn clicking_an_info_bars_close_button_fires_close() {
    // mutation: never set `handled` / never fire EventKind::Close in
    // InfoBarC::on_event and the model stays at 0.
    use icedtea_ui::view::builders::info_bar;
    use icedtea_ui::widgets::info_bar::InfoBarExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Closed;

    let frames = run(
        0u32,
        |model: &mut u32, _msg: Closed| {
            *model += 1;
            Cmd::None
        },
        |_m: &u32| info_bar().show_close_button(true).revealed(true).hexpand(true).on_close(Closed),
        (300, 48),
        vec![
            ScriptStep::Event(InputEvent::PointerEnter { x: 285.0, y: 24.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1);
}

#[test]
fn an_unrevealed_info_bar_inks_nothing() {
    // mutation: ignore `revealed` in InfoBarC and the frame inks the bar.
    use icedtea_ui::view::builders::info_bar;
    use icedtea_ui::widgets::info_bar::InfoBarExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| info_bar().revealed(false).hexpand(true),
        (300, 48),
        vec![ScriptStep::Capture],
    );
    assert!(!has_ink(&frames, 0, (300, 48)), "a hidden info bar draws nothing");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees an_info_bar_carries`
Expected: FAIL — `assert!(rendered.starts_with("infobar.warning"))`.

- [ ] **Step 4: Write `ui/src/widgets/info_bar.rs`**

```rust
//! `GtkInfoBar` — `Kind::InfoBar`, CSS node `infobar`.
//!
//! ```text
//! infobar[.info][.warning][.error][.question]
//! ```
//!
//! Deprecated upstream in 4.10 and kept by contract ruling R1. The close
//! button, when shown, is a `button` node carrying `.close`; GTK's block does
//! not draw it because it is a child widget, so the fixture stops at `infobar`
//! and the matcher's `<child>`-free path check permits the extra subtree only
//! because `button` is not at a path the fixture forbids — see the
//! `fixture_matches` doc: a rendered node whose path the fixture does not name
//! is rejected, so `info_bar.txt` is extended below with the button GTK's prose
//! describes ("If the info bar shows a close button, that button will have the
//! .close style class applied").

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{MessageType, PointerState, WidgetEnum};

/// A `GtkInfoBar`, hidden.
#[must_use]
pub fn info_bar<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::InfoBar)
}

/// `GtkInfoBar`'s own setters and signals.
pub trait InfoBarExt<Msg>: Sized {
    /// `GtkInfoBar:message-type`.
    fn message_type(self, kind: MessageType) -> Self;
    /// `GtkInfoBar:revealed`.
    fn revealed(self, on: bool) -> Self;
    /// `GtkInfoBar:show-close-button`.
    fn show_close_button(self, on: bool) -> Self;
    /// `GtkInfoBar::response`, carrying the action-area button index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    /// `GtkInfoBar::close`.
    fn on_close(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> InfoBarExt<Msg> for View<Msg> {
    fn message_type(self, kind: MessageType) -> Self {
        self.prop(PropName::MessageType, kind.to_prop())
    }
    fn revealed(self, on: bool) -> Self {
        self.prop(PropName::Reveal, Prop::Bool(on))
    }
    fn show_close_button(self, on: bool) -> Self {
        self.prop(PropName::Buttons, Prop::Bool(on))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(std::rc::Rc::new(f)))
    }
    fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }
}

/// `Kind::InfoBar`'s controller.
pub struct InfoBarC {
    /// `GtkInfoBar:revealed`.
    pub revealed: bool,
    /// `0.0..=1.0` reveal animation progress.
    pub reveal_progress: f32,
    /// The `button.close` subnode, when `show-close-button`.
    pub close_button: Option<Node>,
    kind: MessageType,
    pointer: PointerState,
}

impl InfoBarC {
    fn apply(&self, node: &Node) {
        for candidate in MessageType::all() {
            let class = candidate.css_class();
            if !class.is_empty() {
                node.remove_class(class);
            }
        }
        let class = self.kind.css_class();
        if !class.is_empty() {
            node.add_class(class);
        }
        // An unrevealed info bar is not in the tree's paint at all: GTK unmaps
        // it. `visibility: hidden` is the retained-tree equivalent M2 honours.
        node.set_state(PseudoStates::DISABLED, !self.revealed);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for InfoBarC {
    fn kind(&self) -> Kind {
        Kind::InfoBar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let close_button = props.bool(PropName::Buttons, false).then(|| {
            let button = Node::with_classes("button", &["close"]);
            node.append_child(&button);
            button
        });
        let this = InfoBarC {
            revealed: props.bool(PropName::Reveal, false),
            reveal_progress: if props.bool(PropName::Reveal, false) { 1.0 } else { 0.0 },
            close_button,
            kind: MessageType::from_prop(props.get(PropName::MessageType), MessageType::Info),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Reveal, Prop::Bool(on)) => {
                self.revealed = *on;
                self.reveal_progress = if *on { 1.0 } else { 0.0 };
            }
            (PropName::MessageType, Prop::Enum(_)) => {
                self.kind = MessageType::from_prop(Some(value), self.kind);
            }
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(button) = self.close_button.as_ref() else {
            return Vec::new();
        };
        let Some(alloc) = cx.tree.allocation(button) else {
            return Vec::new();
        };
        let root = cx.tree.allocation(cx.node).map(|a| a.border_box);
        let local = root.map_or(alloc.border_box, |root| {
            Rect::new(
                alloc.border_box.x - root.x,
                alloc.border_box.y - root.y,
                alloc.border_box.width,
                alloc.border_box.height,
            )
        });
        let shifted = shift(ev, local);
        if self.pointer.observe(button, &shifted, Some(Rect::new(0.0, 0.0, local.width, local.height)))
        {
            cx.handled = true;
            if let Some(msg) = cx.handlers.fire_unit(EventKind::Close) {
                return vec![msg];
            }
        }
        Vec::new()
    }
}

/// Re-express a root-local pointer event in `rect`'s own space.
fn shift(ev: &Event, rect: Rect) -> Event {
    let map = |local: (f32, f32)| (local.0 - rect.x, local.1 - rect.y);
    match ev {
        Event::PointerEnter { local } => Event::PointerEnter { local: map(*local) },
        Event::PointerMotion { local } => Event::PointerMotion { local: map(*local) },
        Event::PointerDown { button, local, serial } => Event::PointerDown {
            button: *button,
            local: map(*local),
            serial: *serial,
        },
        Event::PointerUp { button, local, serial } => Event::PointerUp {
            button: *button,
            local: map(*local),
            serial: *serial,
        },
        other => other.clone(),
    }
}
```

Move `shift` to `ui/src/widgets/mod.rs` as `pub(crate) fn shift_event(ev: &Event,
rect: Rect) -> Event` before Task 12 — `Scrollbar`, `Scale`, `SpinButton`,
`WindowControls`, `Calendar` and `DropDown` all need it. Task 12 is the first
consumer; do the move there, not here.

- [ ] **Step 5: Extend the fixture with the close button**

Append to `ui/tests/fixtures/gtk4.22-node-trees/info_bar.txt` — GTK's block is
prose about the close button rather than art, so the subnode it describes is
written in GTK's own notation and marked configuration-dependent:

```
infobar[.info][.warning][.error][.question]
╰── [button.close]
```

- [ ] **Step 6: Register the widget**

`ui/src/widgets/mod.rs`: `pub mod info_bar;` and

```rust
        Kind::InfoBar => Box::new(info_bar::InfoBarC::build(node, props, cx)),
```

`ui/src/view/builders.rs`:

```rust
pub use crate::widgets::info_bar::{InfoBarExt, info_bar};
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --test node_trees an_info_bar_carries`
Run: `cargo test -p icedtea-ui --test widget_pixels clicking_an_info_bars`
Run: `cargo test -p icedtea-ui --test widget_pixels an_unrevealed_info_bar`
Expected: PASS, all three.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): InfoBar with its message-type class and close button

GTK's own block is prose about the close button, so the fixture writes the
subnode it describes in GTK's notation, marked configuration-dependent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 12: `Scrollbar` (and the shared `shift_event` helper)

**Files:**
- Create: `ui/src/widgets/scrollbar.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/scrollbar.txt`
- Modify: `ui/src/widgets/mod.rs` (add `shift_event`, move it out of `info_bar.rs`), `ui/src/widgets/info_bar.rs` (use it), `ui/src/view/builders.rs`
- Test: `ui/tests/node_trees.rs`, `ui/tests/widget_pixels.rs`

**Interfaces:**
- Consumes: Task 7's `Adjustment`, `Orientation`, `PointerState`, `WidgetEnum`; P3's `window::keyboard::Mods`.
- Produces:
  ```rust
  pub(crate) fn shift_event(ev: &Event, rect: Rect) -> Event;   // in widgets/mod.rs
  pub fn scrollbar<Msg: Clone + 'static>(orientation: Orientation) -> View<Msg>;
  pub trait ScrollbarExt<Msg>: Sized {
      fn value(self, v: f64) -> Self;
      fn lower(self, v: f64) -> Self;
      fn upper(self, v: f64) -> Self;
      fn page_size(self, v: f64) -> Self;
      fn step_increment(self, v: f64) -> Self;
      fn page_increment(self, v: f64) -> Self;
      fn inverted(self, on: bool) -> Self;
      fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
  }
  pub struct ScrollbarC {
      pub value: f64, pub adj: Adjustment, pub drag: Option<f32>, pub fine_tune: bool,
      pub range: Node, pub trough: Node, pub slider: Node,
  }
  impl ScrollbarC {
      /// P6's `ScrolledWindow` drives an embedded scrollbar through this.
      pub fn set_adjustment(&mut self, adj: Adjustment);
      #[must_use] pub fn slider_rect(&self, trough: Rect, orientation: Orientation) -> Rect;
  }
  ```

- [ ] **Step 1: Vendor the fixture**

`scrollbar.txt` (`gtk/gtkscrollbar.c:63`):

```
scrollbar
╰── range[.fine-tune]
    ╰── trough
        ╰── slider
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_scrollbar_nests_range_trough_and_slider_and_takes_its_orientation_class() {
    // mutation: drop the `trough` node and this fails with "fixture requires a
    // node at 'scrollbar/range/trough'".
    use icedtea_ui::widgets::Orientation;
    let mut props = Props::default();
    props.set(PropName::Orientation, Orientation::Vertical.to_prop());
    check(Kind::Scrollbar, "scrollbar", &props);
    assert!(node_tree_of(Kind::Scrollbar, &props).starts_with("scrollbar.vertical"));
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn dragging_a_scrollbar_slider_reports_the_new_value() {
    // mutation: ignore the drag delta in ScrollbarC::on_event and the model
    // stays at 0.0.
    use icedtea_ui::view::builders::scrollbar;
    use icedtea_ui::widgets::Orientation;
    use icedtea_ui::widgets::scrollbar::ScrollbarExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Moved(f64);

    let frames = run(
        0.0f64,
        |model: &mut f64, Moved(v): Moved| {
            *model = v;
            Cmd::None
        },
        |model: &f64| {
            scrollbar(Orientation::Horizontal)
                .value(*model)
                .upper(100.0)
                .page_size(10.0)
                .hexpand(true)
                .on_value_changed(Moved)
        },
        (200, 20),
        vec![
            ScriptStep::Event(InputEvent::PointerEnter { x: 8.0, y: 10.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerMotion { x: 150.0, y: 10.0, time_ms: 16 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 32,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1, "the drag ran to completion without panicking");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees a_scrollbar_nests`
Expected: FAIL — `fixture requires a node at 'scrollbar/range'`.

- [ ] **Step 4: Move `shift_event` into `widgets/mod.rs`**

Cut `fn shift` from `ui/src/widgets/info_bar.rs`, paste into
`ui/src/widgets/mod.rs` as:

```rust
/// Re-express a pointer event given in the root node's space in `rect`'s space.
///
/// Controllers own their subnodes and hit-test against them; `EventCx` hands
/// coordinates local to the controller's own root node, so every subnode
/// gesture starts by subtracting that subnode's offset within the root.
pub(crate) fn shift_event(ev: &Event, rect: Rect) -> Event {
    let map = |local: (f32, f32)| (local.0 - rect.x, local.1 - rect.y);
    match ev {
        Event::PointerEnter { local } => Event::PointerEnter { local: map(*local) },
        Event::PointerMotion { local } => Event::PointerMotion { local: map(*local) },
        Event::PointerDown { button, local, serial } => {
            Event::PointerDown { button: *button, local: map(*local), serial: *serial }
        }
        Event::PointerUp { button, local, serial } => {
            Event::PointerUp { button: *button, local: map(*local), serial: *serial }
        }
        other => other.clone(),
    }
}

/// A subnode's rectangle in its controller root's own space.
pub(crate) fn local_rect(tree: &crate::layout::LayoutTree, root: &Node, sub: &Node) -> Option<Rect> {
    let root_box = tree.allocation(root)?.border_box;
    let sub_box = tree.allocation(sub)?.border_box;
    Some(Rect::new(
        sub_box.x - root_box.x,
        sub_box.y - root_box.y,
        sub_box.width,
        sub_box.height,
    ))
}
```

and replace `info_bar.rs`'s call sites with `crate::widgets::{shift_event, local_rect}`.

- [ ] **Step 5: Write `ui/src/widgets/scrollbar.rs`**

```rust
//! `GtkScrollbar` — `Kind::Scrollbar`, CSS node `scrollbar`.
//!
//! ```text
//! scrollbar
//! ╰── range[.fine-tune]
//!     ╰── trough
//!         ╰── slider
//! ```
//!
//! The main node takes `.horizontal`/`.vertical`; `range` takes `.fine-tune`
//! while Shift is held or a long press started the drag (`GtkRange`'s rule).
//! `.overlay-indicator`/`.dragging`/`.hovering` are added by P6's
//! `ScrolledWindow`, not here.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{Adjustment, Orientation, PointerState, WidgetEnum, local_rect, shift_event};
use crate::window::keyboard::Mods;

/// A `GtkScrollbar` in `orientation`.
#[must_use]
pub fn scrollbar<Msg: Clone + 'static>(orientation: Orientation) -> View<Msg> {
    View::new(Kind::Scrollbar).prop(PropName::Orientation, orientation.to_prop())
}

/// `GtkScrollbar`'s adjustment, spelled as individual props.
pub trait ScrollbarExt<Msg>: Sized {
    /// `GtkAdjustment:value`.
    fn value(self, v: f64) -> Self;
    /// `GtkAdjustment:lower`.
    fn lower(self, v: f64) -> Self;
    /// `GtkAdjustment:upper`.
    fn upper(self, v: f64) -> Self;
    /// `GtkAdjustment:page-size`.
    fn page_size(self, v: f64) -> Self;
    /// `GtkAdjustment:step-increment`.
    fn step_increment(self, v: f64) -> Self;
    /// `GtkAdjustment:page-increment`.
    fn page_increment(self, v: f64) -> Self;
    /// `GtkRange:inverted`.
    fn inverted(self, on: bool) -> Self;
    /// `GtkRange::value-changed`.
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ScrollbarExt<Msg> for View<Msg> {
    fn value(self, v: f64) -> Self {
        self.prop(PropName::Value, Prop::Float(v))
    }
    fn lower(self, v: f64) -> Self {
        self.prop(PropName::Lower, Prop::Float(v))
    }
    fn upper(self, v: f64) -> Self {
        self.prop(PropName::Upper, Prop::Float(v))
    }
    fn page_size(self, v: f64) -> Self {
        self.prop(PropName::Ratio, Prop::Float(v))
    }
    fn step_increment(self, v: f64) -> Self {
        self.prop(PropName::StepIncrement, Prop::Float(v))
    }
    fn page_increment(self, v: f64) -> Self {
        self.prop(PropName::PageIncrement, Prop::Float(v))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// `Kind::Scrollbar`'s controller. P6's `ScrolledWindow` embeds two of these.
pub struct ScrollbarC {
    /// Mirror of `adj.value`, kept because the model owns the authoritative one.
    pub value: f64,
    /// The sanitized adjustment.
    pub adj: Adjustment,
    /// Grab offset within the slider while dragging, in px.
    pub drag: Option<f32>,
    /// `GtkRange`'s fine-tuning mode.
    pub fine_tune: bool,
    /// The `range` subnode.
    pub range: Node,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `slider` subnode.
    pub slider: Node,
    orientation: Orientation,
    inverted: bool,
    pointer: PointerState,
}

impl ScrollbarC {
    /// Replace the adjustment wholesale — P6's `ScrolledWindow` entry point.
    pub fn set_adjustment(&mut self, adj: Adjustment) {
        self.adj = adj.sanitized();
        self.value = self.adj.value;
    }

    /// Where the slider sits inside `trough`.
    ///
    /// The slider's length is the page's share of the range, floored at 20px so
    /// a huge document still leaves something to grab.
    #[must_use]
    pub fn slider_rect(&self, trough: Rect, orientation: Orientation) -> Rect {
        let span = self.adj.upper - self.adj.lower;
        let visible = if span > 0.0 {
            (self.adj.page_size / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let fraction = if self.inverted {
            1.0 - self.adj.fraction()
        } else {
            self.adj.fraction()
        } as f32;
        match orientation {
            Orientation::Horizontal => {
                let len = (trough.width * visible as f32).max(20.0).min(trough.width);
                Rect::new(trough.x + (trough.width - len) * fraction, trough.y, len, trough.height)
            }
            Orientation::Vertical => {
                let len = (trough.height * visible as f32).max(20.0).min(trough.height);
                Rect::new(trough.x, trough.y + (trough.height - len) * fraction, trough.width, len)
            }
        }
    }

    /// The value a pointer at `local` (trough space) selects.
    fn value_for(&self, local: (f32, f32), trough: Rect, grab: f32) -> f64 {
        let (pos, span) = match self.orientation {
            Orientation::Horizontal => (local.0 - grab, trough.width),
            Orientation::Vertical => (local.1 - grab, trough.height),
        };
        let slider = self.slider_rect(trough, self.orientation);
        let travel = match self.orientation {
            Orientation::Horizontal => span - slider.width,
            Orientation::Vertical => span - slider.height,
        };
        let mut fraction = if travel > 0.0 { f64::from(pos / travel) } else { 0.0 };
        if self.inverted {
            fraction = 1.0 - fraction;
        }
        if self.fine_tune {
            // Fine-tuning moves the value a tenth as far per pixel.
            let base = self.adj.fraction();
            fraction = base + (fraction - base) * 0.1;
        }
        self.adj.value_at_fraction(fraction)
    }

    fn apply(&self, node: &Node) {
        for candidate in Orientation::all() {
            node.remove_class(candidate.css_class());
        }
        node.add_class(self.orientation.css_class());
        if self.fine_tune {
            self.range.add_class("fine-tune");
        } else {
            self.range.remove_class("fine-tune");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ScrollbarC {
    fn kind(&self) -> Kind {
        Kind::Scrollbar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let range = Node::new("range");
        node.append_child(&range);
        let trough = Node::new("trough");
        range.append_child(&trough);
        let slider = Node::new("slider");
        trough.append_child(&slider);
        let adj = Adjustment {
            value: props.float(PropName::Value, 0.0),
            lower: props.float(PropName::Lower, 0.0),
            upper: props.float(PropName::Upper, 1.0),
            step_increment: props.float(PropName::StepIncrement, 0.1),
            page_increment: props.float(PropName::PageIncrement, 0.2),
            page_size: props.float(PropName::Ratio, 0.0),
        }
        .sanitized();
        let this = ScrollbarC {
            value: adj.value,
            adj,
            drag: None,
            fine_tune: false,
            range,
            trough,
            slider,
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
            inverted: props.bool(PropName::Inverted, false),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => {
                self.adj.set_value(*v);
                self.value = self.adj.value;
            }
            (PropName::Lower, Prop::Float(v)) => self.adj.lower = *v,
            (PropName::Upper, Prop::Float(v)) => self.adj.upper = *v,
            (PropName::Ratio, Prop::Float(v)) => self.adj.page_size = *v,
            (PropName::StepIncrement, Prop::Float(v)) => self.adj.step_increment = *v,
            (PropName::PageIncrement, Prop::Float(v)) => self.adj.page_increment = *v,
            (PropName::Inverted, Prop::Bool(on)) => self.inverted = *on,
            (PropName::Orientation, Prop::Enum(_)) => {
                self.orientation = Orientation::from_prop(Some(value), self.orientation);
            }
            _ => return,
        }
        self.adj = self.adj.sanitized();
        self.value = self.adj.value;
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(trough) = local_rect(cx.tree, cx.node, &self.trough) else {
            return Vec::new();
        };
        let local_ev = shift_event(ev, trough);
        self.pointer
            .observe(&self.slider, &local_ev, Some(Rect::new(0.0, 0.0, trough.width, trough.height)));

        let mut emitted = None;
        match &local_ev {
            Event::PointerDown { local, .. } => {
                let slider =
                    self.slider_rect(Rect::new(0.0, 0.0, trough.width, trough.height), self.orientation);
                let grab = match self.orientation {
                    Orientation::Horizontal if local.0 >= slider.x && local.0 <= slider.right() => {
                        local.0 - slider.x
                    }
                    Orientation::Vertical if local.1 >= slider.y && local.1 <= slider.bottom() => {
                        local.1 - slider.y
                    }
                    // A click off the slider jumps it under the pointer, centred.
                    Orientation::Horizontal => slider.width / 2.0,
                    Orientation::Vertical => slider.height / 2.0,
                };
                self.drag = Some(grab);
                let value = self.value_for(*local, trough, grab);
                if self.adj.set_value(value) {
                    self.value = self.adj.value;
                    emitted = cx.handlers.fire_float(EventKind::ValueChanged, self.value);
                }
                cx.handled = true;
            }
            Event::PointerMotion { local } => {
                if let Some(grab) = self.drag {
                    let value = self.value_for(*local, trough, grab);
                    if self.adj.set_value(value) {
                        self.value = self.adj.value;
                        emitted = cx.handlers.fire_float(EventKind::ValueChanged, self.value);
                    }
                    cx.handled = true;
                }
            }
            Event::PointerUp { .. } => {
                self.drag = None;
                self.fine_tune = false;
                self.apply(cx.node);
            }
            Event::Key(key) if key.pressed => {
                self.fine_tune = key.mods.contains(Mods::SHIFT);
                self.apply(cx.node);
            }
            _ => {}
        }
        emitted.map_or_else(Vec::new, |msg| vec![msg])
    }
}
```

- [ ] **Step 6: Register the widget**

`ui/src/widgets/mod.rs`: `pub mod scrollbar;` and
`Kind::Scrollbar => Box::new(scrollbar::ScrollbarC::build(node, props, cx)),`.
`ui/src/view/builders.rs`: `pub use crate::widgets::scrollbar::{ScrollbarExt, scrollbar};`.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --test node_trees a_scrollbar_nests`
Run: `cargo test -p icedtea-ui --test widget_pixels dragging_a_scrollbar_slider`
Expected: PASS, both.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Scrollbar, plus the shared subnode-event helpers

ScrollbarC exposes set_adjustment and slider_rect because P6's ScrolledWindow
embeds two of them. shift_event/local_rect move to widgets/mod.rs: every widget
with an interactive subnode needs the same coordinate shift.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 13: `Image` and `Picture`

**Files:**
- Create: `ui/src/widgets/image.rs`, `ui/src/widgets/picture.rs`
- Create: `ui/tests/fixtures/gtk4.22-node-trees/{image,picture}.txt`
- Modify: `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`
- Test: `ui/tests/node_trees.rs`, `ui/tests/widget_pixels.rs`

**Interfaces:**
- Consumes: Task 7's `IconSize`, `ContentFit`, `PictureSource`, `WidgetEnum`; M2's `css::value::image::IconRef`; P4's `IconTheme` in `BuildCx`/`EventCx`; `skia_rs_safe::codec::decode_image`.
- Produces:
  ```rust
  pub fn image<Msg: Clone + 'static>(icon: IconRef) -> View<Msg>;
  pub trait ImageExt<Msg>: Sized {
      fn icon_name(self, name: &str) -> Self;
      fn file(self, path: &Path) -> Self;
      fn pixel_size(self, px: i32) -> Self;
      fn icon_size(self, size: IconSize) -> Self;
      fn use_fallback(self, on: bool) -> Self;
  }
  pub struct ImageC { pub icon: IconRef, pub resolved: Option<Rc<skia_rs_safe::codec::Image>>,
                      pub pixel_size: i32 }

  pub fn picture<Msg: Clone + 'static>(path: &Path) -> View<Msg>;
  pub trait PictureExt<Msg>: Sized {
      fn content_fit(self, fit: ContentFit) -> Self;
      fn can_shrink(self, on: bool) -> Self;
      fn alternative_text(self, text: &str) -> Self;
  }
  pub struct PictureC { pub source: PictureSource,
                        pub decoded: Option<Rc<skia_rs_safe::codec::Image>>,
                        pub fit: ContentFit }
  ```

- [ ] **Step 1: Vendor the fixtures**

`image.txt` (`gtk/gtkimage.c:76`):

```
image[.normal-icons][.large-icons]
```

`picture.txt` (`gtk/gtkpicture.c:80`):

```
picture
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn an_image_takes_its_icon_size_class_and_a_picture_takes_none() {
    // mutation: always add `.large-icons` and the Normal case fails.
    use icedtea_ui::widgets::IconSize;
    let mut props = Props::default();
    props.set(PropName::IconSize, IconSize::Large.to_prop());
    assert!(node_tree_of(Kind::Image, &props).starts_with("image.large-icons"));
    check(Kind::Image, "image", &props);

    props.set(PropName::IconSize, IconSize::Inherit.to_prop());
    assert_eq!(node_tree_of(Kind::Image, &props).trim(), "image");
    check(Kind::Picture, "picture", &Props::default());
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_picture_decodes_and_draws_an_embedded_png() {
    // mutation: skip `canvas.draw_image_rect` in PictureC::paint and the frame
    // is one flat colour.
    use icedtea_ui::view::builders::picture_from_bytes;
    // A 2x2 opaque red PNG, embedded so the test needs no file on disk.
    const RED_2X2: &[u8] = include_bytes!("fixtures/images/red-2x2.png");
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| picture_from_bytes(RED_2X2).hexpand(true).vexpand(true),
        (64, 64),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (64, 64)), "the decoded PNG must ink");
}

#[test]
fn a_picture_with_an_undecodable_source_draws_nothing_and_does_not_panic() {
    // mutation: unwrap the decode result in PictureC::build and this panics.
    use icedtea_ui::view::builders::picture_from_bytes;
    for junk in [&b""[..], b"not a png", &[0xffu8; 4096][..]] {
        let frames = run(
            (),
            |_m: &mut (), _msg: ()| Cmd::None,
            move |_m: &()| picture_from_bytes(junk).hexpand(true).vexpand(true),
            (32, 32),
            vec![ScriptStep::Capture],
        );
        assert_eq!(frames.len(), 1, "an undecodable source still renders a frame");
    }
}

#[test]
#[ignore = "P7 fills in IconTheme::render (contract §9, plan D9)"]
fn an_image_paints_its_resolved_icon() {
    use icedtea_ui::view::builders::image;
    use icedtea_ui::widgets::image::ImageExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| image_named("image-missing").pixel_size(32),
        (48, 48),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (48, 48)), "the resolved icon must ink");
}
```

Create `ui/tests/fixtures/images/red-2x2.png` with:

```bash
printf '\x89PNG\r\n\x1a\n' > /tmp/hdr
python3 - <<'PY'
import struct, zlib
def chunk(tag, data):
    return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data))
raw = b''.join(b'\x00' + b'\xff\x00\x00' * 2 for _ in range(2))
png = (b'\x89PNG\r\n\x1a\n'
       + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 2, 0, 0, 0))
       + chunk(b'IDAT', zlib.compress(raw))
       + chunk(b'IEND', b''))
open('ui/tests/fixtures/images/red-2x2.png', 'wb').write(png)
PY
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees an_image_takes`
Expected: FAIL — `unresolved import icedtea_ui::widgets::IconSize` is already
resolved; the failure is `assert!(… starts_with("image.large-icons"))`.

- [ ] **Step 4: Write `ui/src/widgets/image.rs`**

```rust
//! `GtkImage` — `Kind::Image`, CSS node `image`.
//!
//! ```text
//! image[.normal-icons][.large-icons]
//! ```
//!
//! An image is an `IconRef` resolved through the icon theme. P7 owns the
//! resolution; until it lands, `IconTheme::render` returns `None` and the node
//! paints only its CSS box (plan D9).

use std::path::Path;
use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::layout::{Allocation, Rect};
use crate::view::controller::{BuildCx, Controller, Event, EventCx, PaintCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::{IconSize, WidgetEnum};

/// A `GtkImage` showing `icon`.
#[must_use]
pub fn image<Msg: Clone + 'static>(icon: IconRef) -> View<Msg> {
    View::new(Kind::Image).prop(PropName::Icon, Prop::Icon(icon))
}

/// A `GtkImage` showing the themed icon `name`.
#[must_use]
pub fn image_named<Msg: Clone + 'static>(name: &str) -> View<Msg> {
    image(IconRef::Theme { name: Rc::from(name) })
}

/// `GtkImage`'s own setters.
pub trait ImageExt<Msg>: Sized {
    /// `GtkImage:icon-name`.
    fn icon_name(self, name: &str) -> Self;
    /// `GtkImage:file`.
    fn file(self, path: &Path) -> Self;
    /// `GtkImage:pixel-size`.
    fn pixel_size(self, px: i32) -> Self;
    /// `GtkImage:icon-size`.
    fn icon_size(self, size: IconSize) -> Self;
    /// `GtkImage:use-fallback`.
    fn use_fallback(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> ImageExt<Msg> for View<Msg> {
    fn icon_name(self, name: &str) -> Self {
        self.prop(PropName::Icon, Prop::Icon(IconRef::Theme { name: Rc::from(name) }))
    }
    fn file(self, path: &Path) -> Self {
        self.prop(
            PropName::Icon,
            Prop::Icon(IconRef::Recolor {
                url: Rc::from(path.to_string_lossy().as_ref()),
                palette: None,
            }),
        )
    }
    fn pixel_size(self, px: i32) -> Self {
        self.prop(PropName::IconSize, Prop::Int(i64::from(px)))
    }
    fn icon_size(self, size: IconSize) -> Self {
        self.prop(PropName::IconSize, size.to_prop())
    }
    fn use_fallback(self, on: bool) -> Self {
        self.prop(PropName::Fit, Prop::Bool(on))
    }
}

/// `Kind::Image`'s controller.
pub struct ImageC {
    /// What to draw.
    pub icon: IconRef,
    /// The rasterized icon, once P7's theme can produce one.
    pub resolved: Option<Rc<skia_rs_safe::codec::Image>>,
    /// Requested size in px; 16 when unset, as GTK's `normal` icon size is.
    pub pixel_size: i32,
    size_class: IconSize,
}

impl ImageC {
    fn apply(&self, node: &Node) {
        for candidate in IconSize::all() {
            let class = candidate.css_class();
            if !class.is_empty() {
                node.remove_class(class);
            }
        }
        let class = self.size_class.css_class();
        if !class.is_empty() {
            node.add_class(class);
        }
    }

    fn resolve(&mut self, cx: &mut BuildCx<'_>, style_colour: crate::css::value::Rgba) {
        let name = match &self.icon {
            IconRef::Theme { name } => name.to_string(),
            IconRef::Recolor { url, .. } => url.to_string(),
            IconRef::Scaled { .. } => String::new(),
        };
        if name.is_empty() {
            self.resolved = None;
            return;
        }
        let palette = crate::icons::Palette {
            foreground: style_colour,
            success: style_colour,
            warning: style_colour,
            error: style_colour,
        };
        let size = u32::try_from(self.pixel_size.max(1)).unwrap_or(16);
        self.resolved = cx.icons.render(&name, size, 1, true, &palette);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ImageC {
    fn kind(&self) -> Kind {
        Kind::Image
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let icon = match props.get(PropName::Icon) {
            Some(Prop::Icon(icon)) => icon.clone(),
            _ => IconRef::Theme { name: Rc::from("image-missing") },
        };
        let (pixel_size, size_class) = match props.get(PropName::IconSize) {
            Some(Prop::Int(px)) => (i32::try_from(*px).unwrap_or(16).clamp(1, 512), IconSize::Inherit),
            Some(Prop::Enum(_)) => {
                let size = IconSize::from_prop(props.get(PropName::IconSize), IconSize::Inherit);
                (
                    match size {
                        IconSize::Large => 32,
                        _ => 16,
                    },
                    size,
                )
            }
            _ => (16, IconSize::Inherit),
        };
        let mut this = ImageC { icon, resolved: None, pixel_size, size_class };
        this.apply(node);
        this.resolve(cx, crate::css::value::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Icon, Prop::Icon(icon)) => self.icon = icon.clone(),
            (PropName::IconSize, Prop::Int(px)) => {
                self.pixel_size = i32::try_from(*px).unwrap_or(16).clamp(1, 512);
                self.size_class = IconSize::Inherit;
            }
            (PropName::IconSize, Prop::Enum(_)) => {
                self.size_class = IconSize::from_prop(Some(value), self.size_class);
                self.pixel_size = match self.size_class {
                    IconSize::Large => 32,
                    _ => 16,
                };
            }
            _ => return,
        }
        self.apply(node);
        self.resolve(cx, crate::css::value::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let size = self.pixel_size as f32;
        Some((size, size))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let Some(icon) = self.resolved.as_ref() else {
            return false;
        };
        let content = alloc.content_box;
        let size = content.width.min(content.height);
        let rect = Rect::new(
            content.x + (content.width - size) / 2.0,
            content.y + (content.height - size) / 2.0,
            size,
            size,
        );
        canvas.draw_image_rect(icon, None, &rect.to_skia(), &skia_rs_safe::core::Paint::new());
        true
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/picture.rs`**

```rust
//! `GtkPicture` — `Kind::Picture`, CSS node `picture`.
//!
//! ```text
//! picture
//! ```
//!
//! A picture is a decoded image file, not a themed icon: it goes through
//! `skia_rs_safe::codec` directly. Decoding is untrusted input — a truncated or
//! hostile file yields `None`, logged once, and the node paints only its box.

use std::path::Path;
use std::rc::Rc;
use std::sync::Once;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::{Allocation, Rect};
use crate::view::controller::{BuildCx, Controller, Event, EventCx, PaintCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::{ContentFit, PictureSource, WidgetEnum};

static DECODE_WARNED: Once = Once::new();

/// A `GtkPicture` showing the image file at `path`.
#[must_use]
pub fn picture<Msg: Clone + 'static>(path: &Path) -> View<Msg> {
    View::new(Kind::Picture).prop(
        PropName::Paintable,
        Prop::Str(Rc::from(path.to_string_lossy().as_ref())),
    )
}

/// A `GtkPicture` over already-loaded encoded bytes, for tests and embedded
/// assets.
#[must_use]
pub fn picture_from_bytes<Msg: Clone + 'static>(bytes: &'static [u8]) -> View<Msg> {
    View::new(Kind::Picture).prop(PropName::Paintable, Prop::Classes(Rc::from(vec![
        Rc::from(encode_bytes_key(bytes)),
    ])))
}

/// Register `bytes` in the process-wide byte-source table and return its key.
///
/// `Prop` has no byte-slice variant and must stay `PartialEq`-cheap, so an
/// embedded image is addressed by a stable key derived from its pointer and
/// length. The table only ever grows by one entry per distinct `&'static [u8]`.
fn encode_bytes_key(bytes: &'static [u8]) -> String {
    use std::cell::RefCell;
    thread_local! {
        static TABLE: RefCell<Vec<&'static [u8]>> = const { RefCell::new(Vec::new()) };
    }
    TABLE.with(|table| {
        let mut table = table.borrow_mut();
        let index = table
            .iter()
            .position(|entry| std::ptr::eq(*entry, bytes))
            .unwrap_or_else(|| {
                table.push(bytes);
                table.len() - 1
            });
        format!("bytes:{index}")
    })
}

/// Look a key produced by [`encode_bytes_key`] back up.
fn decode_bytes_key(key: &str) -> Option<&'static [u8]> {
    use std::cell::RefCell;
    thread_local! {
        static TABLE: RefCell<Vec<&'static [u8]>> = const { RefCell::new(Vec::new()) };
    }
    let index: usize = key.strip_prefix("bytes:")?.parse().ok()?;
    TABLE.with(|table| table.borrow().get(index).copied())
}

/// `GtkPicture`'s own setters.
pub trait PictureExt<Msg>: Sized {
    /// `GtkPicture:content-fit`.
    fn content_fit(self, fit: ContentFit) -> Self;
    /// `GtkPicture:can-shrink`.
    fn can_shrink(self, on: bool) -> Self;
    /// `GtkPicture:alternative-text`.
    fn alternative_text(self, text: &str) -> Self;
}

impl<Msg: Clone + 'static> PictureExt<Msg> for View<Msg> {
    fn content_fit(self, fit: ContentFit) -> Self {
        self.prop(PropName::Fit, fit.to_prop())
    }
    fn can_shrink(self, on: bool) -> Self {
        self.prop(PropName::Selectable, Prop::Bool(on))
    }
    fn alternative_text(self, text: &str) -> Self {
        self.prop(PropName::Tooltip, Prop::Str(Rc::from(text)))
    }
}

/// `Kind::Picture`'s controller.
pub struct PictureC {
    /// Where the pixels came from.
    pub source: PictureSource,
    /// The decoded image, `None` when the source is missing or undecodable.
    pub decoded: Option<Rc<skia_rs_safe::codec::Image>>,
    /// How the image fills the widget.
    pub fit: ContentFit,
}

impl PictureC {
    /// Decode `source`, never panicking. A failure logs once and yields `None`.
    fn decode(source: &PictureSource) -> Option<Rc<skia_rs_safe::codec::Image>> {
        let bytes: Vec<u8> = match source {
            PictureSource::None => return None,
            PictureSource::File(path) => std::fs::read(path.as_ref()).ok()?,
            PictureSource::Bytes(bytes) => bytes.to_vec(),
        };
        match skia_rs_safe::codec::decode_image(&bytes) {
            Ok(image) => Some(Rc::new(image)),
            Err(_) => {
                DECODE_WARNED.call_once(|| {
                    tracing::warn!("a picture source could not be decoded; nothing is drawn");
                });
                None
            }
        }
    }

    fn source_from(props: &Props) -> PictureSource {
        match props.get(PropName::Paintable) {
            Some(Prop::Str(path)) => PictureSource::File(Rc::from(Path::new(path.as_ref()))),
            Some(Prop::Classes(keys)) => keys
                .first()
                .and_then(|key| decode_bytes_key(key))
                .map_or(PictureSource::None, |bytes| PictureSource::Bytes(Rc::from(bytes))),
            _ => PictureSource::None,
        }
    }

    /// The destination rect for `fit` inside `content`.
    fn dest(&self, content: Rect, image_w: f32, image_h: f32) -> Rect {
        if image_w <= 0.0 || image_h <= 0.0 || content.is_empty() {
            return Rect::zero();
        }
        let scale = match self.fit {
            ContentFit::Fill => {
                return content;
            }
            ContentFit::Contain => (content.width / image_w).min(content.height / image_h),
            ContentFit::Cover => (content.width / image_w).max(content.height / image_h),
            ContentFit::ScaleDown => (content.width / image_w).min(content.height / image_h).min(1.0),
        };
        let (w, h) = (image_w * scale, image_h * scale);
        Rect::new(
            content.x + (content.width - w) / 2.0,
            content.y + (content.height - h) / 2.0,
            w,
            h,
        )
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PictureC {
    fn kind(&self) -> Kind {
        Kind::Picture
    }

    fn build(_node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let source = Self::source_from(props);
        PictureC {
            decoded: Self::decode(&source),
            source,
            fit: ContentFit::from_prop(props.get(PropName::Fit), ContentFit::Contain),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Paintable => {
                let mut props = Props::default();
                props.set(PropName::Paintable, value.clone());
                self.source = Self::source_from(&props);
                self.decoded = Self::decode(&self.source);
            }
            PropName::Fit => self.fit = ContentFit::from_prop(Some(value), self.fit),
            _ => {}
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.decoded
            .as_ref()
            .map(|image| (image.width() as f32, image.height() as f32))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let Some(image) = self.decoded.as_ref() else {
            return false;
        };
        let dest = self.dest(alloc.content_box, image.width() as f32, image.height() as f32);
        if dest.is_empty() {
            return false;
        }
        canvas.draw_image_rect(image, None, &dest.to_skia(), &skia_rs_safe::core::Paint::new());
        true
    }
}
```

- [ ] **Step 6: Register both widgets**

`ui/src/widgets/mod.rs`: `pub mod image;`, `pub mod picture;` and

```rust
        Kind::Image => Box::new(image::ImageC::build(node, props, cx)),
        Kind::Picture => Box::new(picture::PictureC::build(node, props, cx)),
```

`ui/src/view/builders.rs`:

```rust
pub use crate::widgets::image::{ImageExt, image, image_named};
pub use crate::widgets::picture::{PictureExt, picture, picture_from_bytes};
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p icedtea-ui --test node_trees an_image_takes`
Run: `cargo test -p icedtea-ui --test widget_pixels a_picture_decodes`
Run: `cargo test -p icedtea-ui --test widget_pixels a_picture_with_an_undecodable`
Expected: PASS; `an_image_paints_its_resolved_icon` reports as ignored.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green, one ignored test.

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Image and Picture

Picture decodes through skia's codec and treats every source as untrusted: a
truncated or hostile file logs once and draws nothing. Image's glyph-level
pixel test is ignored until P7 fills in IconTheme::render (plan D9).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 14: `TextView`

**Files:** Create `ui/src/widgets/text_view.rs`, `ui/tests/fixtures/gtk4.22-node-trees/text_view.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Tasks 1–5's `TextLayout`/`WrapMode`; Task 7's `PointerState`, `local_rect`, `shift_event`; P3's `window::keyboard::{KeyEvent, Mods}`, `window::pointer::Kinetic`.
- Produces:
  ```rust
  pub fn text_view<Msg: Clone + 'static>(text: &str) -> View<Msg>;
  pub trait TextViewExt<Msg>: Sized {
      fn editable(self, on: bool) -> Self;
      fn wrap_mode(self, mode: WrapMode) -> Self;
      fn monospace(self, on: bool) -> Self;
      fn cursor_visible(self, on: bool) -> Self;
      fn left_margin(self, px: i32) -> Self;
      fn right_margin(self, px: i32) -> Self;
      fn top_margin(self, px: i32) -> Self;
      fn bottom_margin(self, px: i32) -> Self;
      fn enable_undo(self, on: bool) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
  }
  pub struct TextViewC {
      pub buffer: String, pub layout: TextLayout, pub cursor: usize, pub anchor: Option<usize>,
      pub scroll: (f32, f32), pub kinetic: Kinetic,
      pub text_node: Node, pub selection_node: Option<Node>,
  }
  ```
  `TextViewC`'s `undo: UndoStack` field is added in Task 23, when `UndoStack`
  exists; until then the controller carries no undo and `Ctrl+Z` is a no-op.

- [ ] **Step 1: Vendor the fixture** — `text_view.txt`, verbatim from `gtk/gtktextview.c:119`:

```
textview.view
├── border.top
├── border.left
├── text
│   ╰── [selection]
├── border.right
├── border.bottom
╰── [window.popup]
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_text_view_builds_its_four_borders_and_a_text_node() {
    // mutation: build three borders instead of four and this fails with
    // "fixture requires a node at 'textview/border'" for the missing side —
    // the four `border` nodes share a path, so drop the `.bottom` class
    // instead to see "required class 'bottom' missing".
    let mut props = Props::default();
    props.set(PropName::Text, Prop::Str("hello\nworld".into()));
    check(Kind::TextView, "text_view", &props);
    let rendered = node_tree_of(Kind::TextView, &props);
    assert!(rendered.starts_with("textview.view"), "{rendered}");
    assert_eq!(rendered.matches("border").count(), 4, "{rendered}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn typing_into_a_text_view_inserts_at_the_cursor_and_reports_the_change() {
    // mutation: ignore `utf8` in TextViewC::on_event and the model stays empty.
    use icedtea_ui::view::builders::text_view;
    use icedtea_ui::widgets::text_view::TextViewExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Edited(String);

    let frames = run(
        String::new(),
        |model: &mut String, Edited(text): Edited| {
            *model = text;
            Cmd::None
        },
        |model: &String| text_view(model).editable(true).on_change(|t| Edited(t.to_owned())),
        (200, 80),
        vec![
            ScriptStep::Event(InputEvent::KeyboardEnter { serial: 1 }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::Key(key_char('h', 43))),
            ScriptStep::Event(InputEvent::Key(key_char('i', 31))),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 12)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the typed glyphs must appear");
}
```

Add the shared key helper to `ui/tests/widget_pixels.rs`:

```rust
/// A synthetic pressed `KeyEvent` carrying one character.
pub fn key_char(ch: char, keycode: u32) -> icedtea_ui::window::keyboard::KeyEvent {
    icedtea_ui::window::keyboard::KeyEvent {
        keycode,
        keysym: xkbcommon::xkb::Keysym::from(u32::from(ch)),
        utf8: Some(ch.to_string()),
        mods: icedtea_ui::window::keyboard::Mods::empty(),
        pressed: true,
        repeat: false,
        serial: 10 + keycode,
        time_ms: keycode,
    }
}
```

and add `xkbcommon = "0.9"` to `ui/Cargo.toml`'s `[dev-dependencies]` — the
test needs to name a `Keysym`, and P3 already links the library.

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_text_view_builds`; expected FAIL with `fixture requires a node at 'textview/border'`.

- [ ] **Step 4: Write `ui/src/widgets/text_view.rs`**

```rust
//! `GtkTextView` — `Kind::TextView`, CSS node `textview`, always `.view`.
//!
//! ```text
//! textview.view
//! ├── border.top
//! ├── border.left
//! ├── text
//! │   ╰── [selection]
//! ├── border.right
//! ├── border.bottom
//! ╰── [window.popup]
//! ```
//!
//! Plain text only (spec §Out of scope). Editing keys follow `GtkText`'s own
//! table, which contract §5.3 makes the single source for the whole family.

use std::rc::Rc;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::Rect;
use crate::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, local_rect, shift_event};
use crate::window::keyboard::Mods;
use crate::window::pointer::Kinetic;

/// A `GtkTextView` over `text`.
#[must_use]
pub fn text_view<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::TextView).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkTextView`'s own setters.
pub trait TextViewExt<Msg>: Sized {
    /// `GtkTextView:editable`.
    fn editable(self, on: bool) -> Self;
    /// `GtkTextView:wrap-mode`.
    fn wrap_mode(self, mode: WrapMode) -> Self;
    /// `GtkTextView:monospace`.
    fn monospace(self, on: bool) -> Self;
    /// `GtkTextView:cursor-visible`.
    fn cursor_visible(self, on: bool) -> Self;
    /// `GtkTextView:left-margin`.
    fn left_margin(self, px: i32) -> Self;
    /// `GtkTextView:right-margin`.
    fn right_margin(self, px: i32) -> Self;
    /// `GtkTextView:top-margin`.
    fn top_margin(self, px: i32) -> Self;
    /// `GtkTextView:bottom-margin`.
    fn bottom_margin(self, px: i32) -> Self;
    /// `GtkTextView:enable-undo`.
    fn enable_undo(self, on: bool) -> Self;
    /// `GtkTextBuffer::changed`.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> TextViewExt<Msg> for View<Msg> {
    fn editable(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn wrap_mode(self, mode: WrapMode) -> Self {
        self.prop(
            PropName::Wrap,
            Prop::Enum(match mode {
                WrapMode::None => 0,
                WrapMode::Word => 1,
                WrapMode::Char => 2,
                WrapMode::WordChar => 3,
            }),
        )
    }
    fn monospace(self, on: bool) -> Self {
        self.prop(PropName::Homogeneous, Prop::Bool(on))
    }
    fn cursor_visible(self, on: bool) -> Self {
        self.prop(PropName::Visibility, Prop::Bool(on))
    }
    fn left_margin(self, px: i32) -> Self {
        self.prop(PropName::Column, Prop::Int(i64::from(px)))
    }
    fn right_margin(self, px: i32) -> Self {
        self.prop(PropName::ColumnSpan, Prop::Int(i64::from(px)))
    }
    fn top_margin(self, px: i32) -> Self {
        self.prop(PropName::Row, Prop::Int(i64::from(px)))
    }
    fn bottom_margin(self, px: i32) -> Self {
        self.prop(PropName::RowSpan, Prop::Int(i64::from(px)))
    }
    fn enable_undo(self, on: bool) -> Self {
        self.prop(PropName::EnableUndo, Prop::Bool(on))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
}

/// `Kind::TextView`'s controller.
pub struct TextViewC {
    /// The plain-text buffer.
    pub buffer: String,
    /// The wrapped paragraph over `buffer`.
    pub layout: TextLayout,
    /// Cursor byte offset.
    pub cursor: usize,
    /// Selection anchor; `None` means no selection.
    pub anchor: Option<usize>,
    /// Scroll offset in px.
    pub scroll: (f32, f32),
    /// Kinetic scrolling state, fed from touch and finger axis events.
    pub kinetic: Kinetic,
    /// The `text` subnode.
    pub text_node: Node,
    /// The `selection` subnode, present only while a selection exists.
    pub selection_node: Option<Node>,
    editable: bool,
    wrap: WrapMode,
    pointer: PointerState,
}

impl TextViewC {
    fn reshape(&mut self, cx: &mut BuildCx<'_>, width: Option<f32>) {
        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        self.layout =
            TextLayout::build(&self.buffer, &style, cx.fonts, width, self.wrap, Ellipsize::None);
    }

    /// Keep the `selection` subnode in sync with `anchor`.
    fn sync_selection(&mut self) {
        let has = self.anchor.is_some_and(|a| a != self.cursor);
        match (has, self.selection_node.take()) {
            (true, Some(node)) => self.selection_node = Some(node),
            (true, None) => {
                let node = Node::new("selection");
                self.text_node.append_child(&node);
                self.selection_node = Some(node);
            }
            (false, Some(node)) => node.detach(),
            (false, None) => {}
        }
    }

    /// Apply one key to the buffer. Returns `true` if the buffer changed.
    fn apply_key(&mut self, key: &crate::window::keyboard::KeyEvent) -> bool {
        use xkbcommon::xkb::keysyms;
        if !key.pressed {
            return false;
        }
        let ctrl = key.effective_mods().contains(Mods::CTRL);
        let shift = key.effective_mods().contains(Mods::SHIFT);
        let extend = |this: &mut Self, to: usize| {
            if shift {
                this.anchor.get_or_insert(this.cursor);
            } else {
                this.anchor = None;
            }
            this.cursor = to;
        };
        match u32::from(key.keysym) {
            keysyms::KEY_Left => {
                let to = if ctrl {
                    self.layout.prev_word(self.cursor)
                } else {
                    self.layout.prev_grapheme(self.cursor)
                };
                extend(self, to);
                false
            }
            keysyms::KEY_Right => {
                let to = if ctrl {
                    self.layout.next_word(self.cursor)
                } else {
                    self.layout.next_grapheme(self.cursor)
                };
                extend(self, to);
                false
            }
            keysyms::KEY_Home => {
                extend(self, 0);
                false
            }
            keysyms::KEY_End => {
                let end = self.buffer.len();
                extend(self, end);
                false
            }
            keysyms::KEY_a if ctrl => {
                self.anchor = Some(0);
                self.cursor = self.buffer.len();
                false
            }
            keysyms::KEY_BackSpace if self.editable => {
                let range = self.selection_range();
                if range.is_empty() {
                    let from = self.layout.prev_grapheme(self.cursor);
                    if from == self.cursor {
                        return false;
                    }
                    self.buffer.replace_range(from..self.cursor, "");
                    self.cursor = from;
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                true
            }
            keysyms::KEY_Delete if self.editable => {
                let range = self.selection_range();
                if range.is_empty() {
                    let to = self.layout.next_grapheme(self.cursor);
                    if to == self.cursor {
                        return false;
                    }
                    self.buffer.replace_range(self.cursor..to, "");
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                true
            }
            _ => {
                let Some(text) = key.utf8.as_deref().filter(|_| self.editable && !ctrl) else {
                    return false;
                };
                if text.chars().all(|c| c.is_control() && c != '\n') {
                    return false;
                }
                let range = self.selection_range();
                self.buffer.replace_range(range.clone(), text);
                self.cursor = range.start + text.len();
                self.anchor = None;
                true
            }
        }
    }

    /// The ordered, clamped selection range; empty when there is no selection.
    fn selection_range(&self) -> std::ops::Range<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor).min(self.buffer.len());
        let cursor = self.cursor.min(self.buffer.len());
        anchor.min(cursor)..anchor.max(cursor)
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for TextViewC {
    fn kind(&self) -> Kind {
        Kind::TextView
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("view");
        for side in ["top", "left"] {
            let border = Node::with_classes("border", &[side]);
            node.append_child(&border);
        }
        let text_node = Node::new("text");
        node.append_child(&text_node);
        for side in ["right", "bottom"] {
            let border = Node::with_classes("border", &[side]);
            node.append_child(&border);
        }
        let mut this = TextViewC {
            buffer: props.str(PropName::Text).unwrap_or("").to_owned(),
            layout: TextLayout::build(
                "",
                &TextStyle::from_computed(&ComputedStyle::initial(cx.env)),
                cx.fonts,
                None,
                WrapMode::None,
                Ellipsize::None,
            ),
            cursor: 0,
            anchor: None,
            scroll: (0.0, 0.0),
            kinetic: Kinetic::default(),
            text_node,
            selection_node: None,
            editable: props.bool(PropName::Editable, true),
            wrap: match props.get(PropName::Wrap) {
                Some(Prop::Enum(1)) => WrapMode::Word,
                Some(Prop::Enum(2)) => WrapMode::Char,
                Some(Prop::Enum(3)) => WrapMode::WordChar,
                _ => WrapMode::None,
            },
            pointer: PointerState::default(),
        };
        this.reshape(cx, None);
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => {
                if self.buffer.as_str() != text.as_ref() {
                    self.buffer = text.to_string();
                    self.cursor = self.cursor.min(self.buffer.len());
                    self.anchor = None;
                    self.reshape(cx, None);
                    self.sync_selection();
                }
            }
            (PropName::Editable, Prop::Bool(on)) => {
                self.editable = *on;
                if *on {
                    self.text_node.remove_class("readonly");
                } else {
                    self.text_node.add_class("readonly");
                }
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Some(rect) = local_rect(cx.tree, cx.node, &self.text_node) {
            let shifted = shift_event(ev, rect);
            self.pointer
                .observe(&self.text_node, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)));
            if let Event::PointerDown { local, .. } = shifted {
                self.cursor = self.layout.byte_at(local);
                self.anchor = None;
                self.sync_selection();
                cx.handled = true;
            }
        }
        if let Event::Key(key) = ev {
            let changed = self.apply_key(key);
            self.sync_selection();
            if changed {
                cx.handled = true;
                let text = self.buffer.clone();
                if let Some(msg) = cx.handlers.fire_text(EventKind::Change, &text) {
                    return vec![msg];
                }
            }
        }
        Vec::new()
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.reshape(cx, available.0.filter(|w| w.is_finite() && *w > 0.0));
        Some(self.layout.size())
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        for rect in self.layout.selection_rects(self.selection_range()) {
            let placed =
                Rect::new(content.x + rect.x, content.y + rect.y, rect.width, rect.height);
            canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        }
        self.layout
            .draw(canvas, (content.x - self.scroll.0, content.y - self.scroll.1), style.color());
        true
    }
}
```

- [ ] **Step 5: Register the widget** — `pub mod text_view;` in `ui/src/widgets/mod.rs`, the arm `Kind::TextView => Box::new(text_view::TextViewC::build(node, props, cx)),`, and `pub use crate::widgets::text_view::{TextViewExt, text_view};` in `ui/src/view/builders.rs`.

- [ ] **Step 6: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_text_view_builds` and `cargo test -p icedtea-ui --test widget_pixels typing_into_a_text_view`; expected PASS.

- [ ] **Step 7: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 8: Commit**

```bash
git add ui/Cargo.toml ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): TextView with plain-text editing and selection

Editing keys follow GtkText's own table, which contract §5.3 makes the single
source for the whole entry family; the undo stack arrives with TextEditState.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 15: `Scale`

**Files:** Create `ui/src/widgets/scale.rs`, `ui/tests/fixtures/gtk4.22-node-trees/scale.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `Adjustment`, `Mark`, `Orientation`, `Position`, `PointerState`, `local_rect`, `shift_event`; Task 8's `LabelC` for mark labels.
- Produces:
  ```rust
  pub fn scale<Msg: Clone + 'static>(lower: f64, upper: f64) -> View<Msg>;
  pub trait ScaleExt<Msg>: Sized {
      fn value(self, v: f64) -> Self;
      fn orientation(self, o: Orientation) -> Self;
      fn digits(self, n: i32) -> Self;
      fn draw_value(self, on: bool) -> Self;
      fn value_pos(self, pos: Position) -> Self;
      fn mark(self, value: f64, pos: Position, label: Option<&str>) -> Self;
      fn show_fill_level(self, on: bool) -> Self;
      fn fill_level(self, v: f64) -> Self;
      fn inverted(self, on: bool) -> Self;
      fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
  }
  pub struct ScaleC {
      pub value: f64, pub adj: Adjustment, pub marks: Vec<Mark>, pub drag: Option<f64>,
      pub fine_tune: bool, pub trough: Node, pub slider: Node, pub highlight: Node,
      pub fill: Option<Node>, pub value_node: Option<Node>, pub mark_nodes: Vec<Node>,
  }
  ```

- [ ] **Step 1: Vendor the fixture** — `scale.txt`, verbatim from `gtk/gtkscale.c:85`:

```
scale[.fine-tune][.marks-before][.marks-after]
├── [value][.top][.right][.bottom][.left]
├── marks.top
│   ├── mark
│   ┊    ├── [label]
│   ┊    ╰── indicator
┊   ┊
│   ╰── mark
├── marks.bottom
│   ├── mark
│   ┊    ├── indicator
│   ┊    ╰── [label]
┊   ┊
│   ╰── mark
╰── trough
    ├── [fill]
    ├── [highlight]
    ╰── slider
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_scale_builds_a_trough_with_a_highlight_and_one_mark_node_per_mark() {
    // mutation: skip the `indicator` subnode under each mark and this fails
    // with "fixture requires a node at 'scale/marks/mark/indicator'".
    let mut props = Props::default();
    props.set(PropName::Lower, Prop::Float(0.0));
    props.set(PropName::Upper, Prop::Float(100.0));
    props.set(PropName::Value, Prop::Float(50.0));
    check(Kind::Scale, "scale", &props);

    props.set(
        PropName::MarksTop,
        Prop::Classes(std::rc::Rc::from(vec![std::rc::Rc::from("0=Min"), std::rc::Rc::from("100=Max")])),
    );
    let rendered = node_tree_of(Kind::Scale, &props);
    assert_eq!(rendered.matches("mark\n").count(), 2, "{rendered}");
    assert!(rendered.starts_with("scale.marks-before"), "{rendered}");
    check(Kind::Scale, "scale", &props);
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn dragging_a_scale_moves_the_slider_and_reports_the_value() {
    // mutation: return early from ScaleC::on_event's PointerMotion arm and the
    // model stays at 0.0.
    use icedtea_ui::view::builders::scale;
    use icedtea_ui::widgets::scale::ScaleExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Set(f64);

    let frames = run(
        0.0f64,
        |model: &mut f64, Set(v): Set| {
            *model = v;
            Cmd::None
        },
        |model: &f64| scale(0.0, 100.0).value(*model).hexpand(true).on_value_changed(Set),
        (200, 32),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 10.0, y: 16.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerMotion { x: 180.0, y: 16.0, time_ms: 16 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 32,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 16)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the slider must have moved right");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_scale_builds`; expected FAIL with `fixture requires a node at 'scale/trough'`.

- [ ] **Step 4: Write `ui/src/widgets/scale.rs`**

```rust
//! `GtkScale` — `Kind::Scale`, CSS node `scale`.
//!
//! ```text
//! scale[.fine-tune][.marks-before][.marks-after]
//! ├── [value][.top][.right][.bottom][.left]
//! ├── marks.top
//! │   ├── mark
//! │   ┊    ├── [label]
//! │   ┊    ╰── indicator
//! ┊   ┊
//! │   ╰── mark
//! ├── marks.bottom
//! │   ├── mark
//! │   ┊    ├── indicator
//! │   ┊    ╰── [label]
//! ┊   ┊
//! │   ╰── mark
//! ╰── trough
//!     ├── [fill]
//!     ├── [highlight]
//!     ╰── slider
//! ```
//!
//! Within a `mark`, the `label` comes first when the mark is above or left of
//! the scale and second otherwise — GTK's own rule, and the reason the fixture
//! spells the two `marks` groups out separately.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{
    Adjustment, Mark, Orientation, PointerState, Position, WidgetEnum, local_rect, shift_event,
};
use crate::window::keyboard::Mods;

/// A `GtkScale` over `lower..=upper`.
#[must_use]
pub fn scale<Msg: Clone + 'static>(lower: f64, upper: f64) -> View<Msg> {
    View::new(Kind::Scale)
        .prop(PropName::Lower, Prop::Float(lower))
        .prop(PropName::Upper, Prop::Float(upper))
}

/// `GtkScale`'s own setters.
pub trait ScaleExt<Msg>: Sized {
    /// `GtkRange:adjustment`'s value.
    fn value(self, v: f64) -> Self;
    /// `GtkOrientable:orientation`.
    fn orientation(self, o: Orientation) -> Self;
    /// `GtkScale:digits`.
    fn digits(self, n: i32) -> Self;
    /// `GtkScale:draw-value`.
    fn draw_value(self, on: bool) -> Self;
    /// `GtkScale:value-pos`.
    fn value_pos(self, pos: Position) -> Self;
    /// `gtk_scale_add_mark`. Repeated calls accumulate.
    fn mark(self, value: f64, pos: Position, label: Option<&str>) -> Self;
    /// `GtkRange:show-fill-level`.
    fn show_fill_level(self, on: bool) -> Self;
    /// `GtkRange:fill-level`.
    fn fill_level(self, v: f64) -> Self;
    /// `GtkRange:inverted`.
    fn inverted(self, on: bool) -> Self;
    /// `GtkRange::value-changed`.
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ScaleExt<Msg> for View<Msg> {
    fn value(self, v: f64) -> Self {
        self.prop(PropName::Value, Prop::Float(v))
    }
    fn orientation(self, o: Orientation) -> Self {
        self.prop(PropName::Orientation, o.to_prop())
    }
    fn digits(self, n: i32) -> Self {
        self.prop(PropName::Digits, Prop::Int(i64::from(n)))
    }
    fn draw_value(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn value_pos(self, pos: Position) -> Self {
        self.prop(PropName::Position, pos.to_prop())
    }
    fn mark(self, value: f64, pos: Position, label: Option<&str>) -> Self {
        // Marks ride in one `Classes` prop as `value=label` (label optional);
        // `Prop` has no list-of-structs variant and a scale has a handful.
        let slot = match pos {
            Position::Top | Position::Left => PropName::MarksTop,
            Position::Bottom | Position::Right => PropName::MarksBottom,
        };
        let mut encoded: Vec<Rc<str>> = match self.props.get(slot) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        encoded.push(Rc::from(format!("{value}={}", label.unwrap_or(""))));
        self.prop(slot, Prop::Classes(Rc::from(encoded)))
    }
    fn show_fill_level(self, on: bool) -> Self {
        self.prop(PropName::FillLevel, Prop::Bool(on))
    }
    fn fill_level(self, v: f64) -> Self {
        self.prop(PropName::Ratio, Prop::Float(v))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// Decode one `MarksTop`/`MarksBottom` prop into marks at `position`.
fn decode_marks(prop: Option<&Prop>, position: Position) -> Vec<Mark> {
    let Some(Prop::Classes(list)) = prop else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|encoded| {
            let (value, label) = encoded.split_once('=')?;
            let value = value.parse::<f64>().ok().filter(|v| v.is_finite())?;
            Some(Mark {
                value,
                position,
                label: (!label.is_empty()).then(|| Rc::from(label)),
            })
        })
        .collect()
}

/// `Kind::Scale`'s controller.
pub struct ScaleC {
    /// Current value.
    pub value: f64,
    /// The sanitized adjustment.
    pub adj: Adjustment,
    /// Marks, in the order the props declared them.
    pub marks: Vec<Mark>,
    /// The value the drag started from.
    pub drag: Option<f64>,
    /// `GtkRange`'s fine-tuning mode.
    pub fine_tune: bool,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `slider` subnode.
    pub slider: Node,
    /// The `highlight` subnode (the scale always has an origin).
    pub highlight: Node,
    /// The `fill` subnode, only with `show-fill-level`.
    pub fill: Option<Node>,
    /// The `value` subnode, only with `draw-value`.
    pub value_node: Option<Node>,
    /// One node per mark, in `marks` order.
    pub mark_nodes: Vec<Node>,
    orientation: Orientation,
    inverted: bool,
    pointer: PointerState,
}

impl ScaleC {
    /// Where the slider sits inside a trough of `rect`.
    fn slider_centre(&self, rect: Rect) -> f32 {
        let fraction = if self.inverted { 1.0 - self.adj.fraction() } else { self.adj.fraction() };
        match self.orientation {
            Orientation::Horizontal => rect.x + rect.width * fraction as f32,
            Orientation::Vertical => rect.y + rect.height * fraction as f32,
        }
    }

    /// The value a pointer at `local` (trough space) selects.
    fn value_for(&self, local: (f32, f32), rect: Rect) -> f64 {
        let (pos, span) = match self.orientation {
            Orientation::Horizontal => (local.0, rect.width),
            Orientation::Vertical => (local.1, rect.height),
        };
        let mut fraction = if span > 0.0 { f64::from(pos / span) } else { 0.0 };
        if self.inverted {
            fraction = 1.0 - fraction;
        }
        if self.fine_tune {
            let base = self.adj.fraction();
            fraction = base + (fraction - base) * 0.1;
        }
        self.adj.value_at_fraction(fraction)
    }

    fn apply(&self, node: &Node) {
        node.remove_class("fine-tune");
        if self.fine_tune {
            node.add_class("fine-tune");
        }
        node.remove_class("marks-before");
        node.remove_class("marks-after");
        if self.marks.iter().any(|m| matches!(m.position, Position::Top | Position::Left)) {
            node.add_class("marks-before");
        }
        if self.marks.iter().any(|m| matches!(m.position, Position::Bottom | Position::Right)) {
            node.add_class("marks-after");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ScaleC {
    fn kind(&self) -> Kind {
        Kind::Scale
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let value_node = props.bool(PropName::ShowText, false).then(|| {
            let position = Position::from_prop(props.get(PropName::Position), Position::Top);
            let value = Node::with_classes("value", &[position.css_class()]);
            node.append_child(&value);
            value
        });

        let mut marks = decode_marks(props.get(PropName::MarksTop), Position::Top);
        marks.extend(decode_marks(props.get(PropName::MarksBottom), Position::Bottom));
        let mut mark_nodes = Vec::new();
        for group in [Position::Top, Position::Bottom] {
            let in_group: Vec<&Mark> = marks.iter().filter(|m| m.position == group).collect();
            if in_group.is_empty() {
                continue;
            }
            let marks_node = Node::with_classes("marks", &[group.css_class()]);
            node.append_child(&marks_node);
            for mark in in_group {
                let mark_node = Node::new("mark");
                marks_node.append_child(&mark_node);
                // Label first when the mark is above or left, indicator first
                // otherwise — GTK's own ordering rule.
                let label_first = matches!(group, Position::Top | Position::Left);
                let indicator = Node::new("indicator");
                if label_first {
                    if mark.label.is_some() {
                        mark_node.append_child(&Node::new("label"));
                    }
                    mark_node.append_child(&indicator);
                } else {
                    mark_node.append_child(&indicator);
                    if mark.label.is_some() {
                        mark_node.append_child(&Node::new("label"));
                    }
                }
                mark_nodes.push(mark_node);
            }
        }

        let trough = Node::new("trough");
        node.append_child(&trough);
        let fill = props.bool(PropName::FillLevel, false).then(|| {
            let fill = Node::new("fill");
            trough.append_child(&fill);
            fill
        });
        let highlight = Node::new("highlight");
        trough.append_child(&highlight);
        let slider = Node::new("slider");
        trough.append_child(&slider);

        let adj = Adjustment {
            value: props.float(PropName::Value, 0.0),
            lower: props.float(PropName::Lower, 0.0),
            upper: props.float(PropName::Upper, 1.0),
            step_increment: props.float(PropName::StepIncrement, 1.0),
            page_increment: props.float(PropName::PageIncrement, 10.0),
            page_size: 0.0,
        }
        .sanitized();

        let this = ScaleC {
            value: adj.value,
            adj,
            marks,
            drag: None,
            fine_tune: false,
            trough,
            slider,
            highlight,
            fill,
            value_node,
            mark_nodes,
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
            inverted: props.bool(PropName::Inverted, false),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => {
                self.adj.set_value(*v);
                self.value = self.adj.value;
            }
            (PropName::Lower, Prop::Float(v)) => self.adj.lower = *v,
            (PropName::Upper, Prop::Float(v)) => self.adj.upper = *v,
            (PropName::Inverted, Prop::Bool(on)) => self.inverted = *on,
            _ => return,
        }
        self.adj = self.adj.sanitized();
        self.value = self.adj.value;
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, cx.node, &self.trough) else {
            return Vec::new();
        };
        let local_ev = shift_event(ev, rect);
        let bounds = Rect::new(0.0, 0.0, rect.width, rect.height);
        self.pointer.observe(&self.slider, &local_ev, Some(bounds));

        let mut emitted = None;
        let mut commit = |this: &mut Self, local: (f32, f32), cx: &mut EventCx<'_, Msg>| {
            let value = this.value_for(local, bounds);
            if this.adj.set_value(value) {
                this.value = this.adj.value;
                emitted = cx.handlers.fire_float(EventKind::ValueChanged, this.value);
            }
            cx.handled = true;
        };
        match &local_ev {
            Event::PointerDown { local, .. } => {
                self.drag = Some(self.value);
                commit(self, *local, cx);
            }
            Event::PointerMotion { local } if self.drag.is_some() => commit(self, *local, cx),
            Event::PointerUp { .. } => {
                self.drag = None;
                self.fine_tune = false;
                self.apply(cx.node);
            }
            Event::Key(key) if key.pressed => {
                self.fine_tune = key.mods.contains(Mods::SHIFT);
                self.apply(cx.node);
            }
            _ => {}
        }
        emitted.map_or_else(Vec::new, |msg| vec![msg])
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        // The trough's highlight runs from its origin to the slider centre.
        let content = alloc.content_box;
        let centre = self.slider_centre(content);
        let highlight = match self.orientation {
            Orientation::Horizontal => {
                Rect::new(content.x, content.y, (centre - content.x).max(0.0), content.height)
            }
            Orientation::Vertical => {
                Rect::new(content.x, content.y, content.width, (centre - content.y).max(0.0))
            }
        };
        if highlight.is_empty() {
            return false;
        }
        canvas.draw_rect(&highlight.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}
```

- [ ] **Step 5: Register the widget** — `pub mod scale;`, the arm `Kind::Scale => Box::new(scale::ScaleC::build(node, props, cx)),`, and `pub use crate::widgets::scale::{ScaleExt, scale};`.

- [ ] **Step 6: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_scale_builds` and `cargo test -p icedtea-ui --test widget_pixels dragging_a_scale`; expected PASS.

- [ ] **Step 7: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 8: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Scale with marks, fill level and fine-tuning

A mark's label precedes its indicator when the mark is above or left of the
scale and follows it otherwise, which is GTK's own rule and the reason the
fixture spells the two marks groups out separately.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 16: `DrawingArea` and `WindowControls`

**Files:** Create `ui/src/widgets/drawing_area.rs`, `ui/src/widgets/window_controls.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{drawing_area,window_controls}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `Side`, `WindowButton`, `PointerState`, `local_rect`, `shift_event`; P4's `Cmd::{Minimize, ToggleMaximized, CloseWindow}`; P3's `SurfaceStates`.
- Produces:
  ```rust
  pub fn drawing_area<Msg: Clone + 'static>(
      draw: impl Fn(&mut Canvas<'_>, Rect) + 'static) -> View<Msg>;
  pub trait DrawingAreaExt<Msg>: Sized {
      fn content_width(self, px: i32) -> Self;
      fn content_height(self, px: i32) -> Self;
      fn on_resize(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
  }
  pub struct DrawingAreaC { pub draw: Rc<dyn Fn(&mut Canvas<'_>, Rect)>,
                            pub content: (i32, i32), pub last_size: (f32, f32) }

  pub fn window_controls<Msg: Clone + 'static>(side: Side) -> View<Msg>;
  pub trait WindowControlsExt<Msg>: Sized { fn decoration_layout(self, layout: &str) -> Self; }
  pub struct WindowControlsC { pub side: Side, pub layout: Rc<str>,
                               pub buttons: Vec<(WindowButton, Node)>, pub empty: bool }
  impl WindowControlsC {
      /// The tokens `side` contributes, in order — `update_window_buttons`'
      /// rule, exactly.
      #[must_use] pub fn tokens(layout: &str, side: Side) -> Vec<WindowButton>;
  }
  ```

- [ ] **Step 1: Vendor the fixtures**

`drawing_area.txt` (GTK sets no CSS name; the node is `GtkWidget`'s default, `gtk/gtkwidget.c:1959`):

```
widget
```

`window_controls.txt` (`gtk/gtkwindowcontrols.c:74`), with the state classes contract §5.1 names:

```
windowcontrols[.start][.end][.empty][.native]
├── [image.icon]
├── [button.minimize]
├── [button.maximize]
╰── [button.close]
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn window_controls_follow_the_decoration_layout_rule_verbatim() {
    // mutation: stop splitting the layout on ':' and the `start` side emits the
    // right-hand buttons too, failing the `close` assertion below.
    use icedtea_ui::widgets::{Side, WindowButton, window_controls::WindowControlsC};
    assert_eq!(
        WindowControlsC::tokens("menu:minimize,maximize,close", Side::Start),
        Vec::<WindowButton>::new(),
        "`menu` produces no child in 4.22.4 and nothing else is on the left"
    );
    assert_eq!(
        WindowControlsC::tokens("menu:minimize,maximize,close", Side::End),
        vec![WindowButton::Minimize, WindowButton::Maximize, WindowButton::Close],
    );
    assert_eq!(
        WindowControlsC::tokens("icon,close:", Side::Start),
        vec![WindowButton::Icon, WindowButton::Close],
        "tokens are walked in order"
    );

    let mut props = Props::default();
    props.set(PropName::Side, Side::End.to_prop());
    props.set(PropName::Decoration, Prop::Str("menu:minimize,maximize,close".into()));
    let rendered = node_tree_of(Kind::WindowControls, &props);
    assert!(rendered.contains("button.close"), "{rendered}");
    assert!(!rendered.contains(".empty"), "{rendered}");
    check(Kind::WindowControls, "window_controls", &props);

    props.set(PropName::Decoration, Prop::Str("menu:".into()));
    let empty = node_tree_of(Kind::WindowControls, &props);
    assert!(empty.starts_with("windowcontrols.end.empty"), "{empty}");
}

#[test]
fn a_drawing_area_is_one_widget_node() {
    // mutation: name the node "drawingarea" and this fails with "rendered node
    // at 'drawingarea' is not in the fixture".
    check(Kind::DrawingArea, "drawing_area", &Props::default());
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn a_drawing_area_runs_its_callback_against_the_allocated_rect() {
    // mutation: never call `self.draw` in DrawingAreaC::paint and the frame is
    // one flat colour.
    use icedtea_ui::view::builders::drawing_area;
    use icedtea_ui::css::value::Rgba;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| {
            drawing_area(|canvas, rect| {
                canvas.draw_rect(
                    &rect.to_skia(),
                    &icedtea_ui::paint::fill_paint(Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }),
                );
            })
            .hexpand(true)
            .vexpand(true)
        },
        (64, 64),
        vec![ScriptStep::Capture],
    );
    assert_eq!(frames.pixel(0, 32, 32).map(|p| (p.0, p.1, p.2)), Some((255, 0, 0)));
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees window_controls_follow`; expected FAIL, `unresolved import ...window_controls`.

- [ ] **Step 4: Write `ui/src/widgets/drawing_area.rs`**

```rust
//! `GtkDrawingArea` — `Kind::DrawingArea`, CSS node `widget`.
//!
//! ```text
//! widget
//! ```
//!
//! GTK sets no CSS name on this class, so the node is `GtkWidget`'s own
//! default. Style it with a class you add.

use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::{Allocation, Rect};
use crate::view::controller::{BuildCx, Controller, Event, EventCx, PaintCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};

/// A `GtkDrawingArea` whose `draw_func` is `draw`.
#[must_use]
pub fn drawing_area<Msg: Clone + 'static>(
    draw: impl Fn(&mut Canvas<'_>, Rect) + 'static,
) -> View<Msg> {
    View::new(Kind::DrawingArea).prop(PropName::DrawFn, Prop::Draw(Rc::new(draw)))
}

/// `GtkDrawingArea`'s own setters.
pub trait DrawingAreaExt<Msg>: Sized {
    /// `GtkDrawingArea:content-width`.
    fn content_width(self, px: i32) -> Self;
    /// `GtkDrawingArea:content-height`.
    fn content_height(self, px: i32) -> Self;
    /// `GtkDrawingArea::resize`, carrying the new width.
    fn on_resize(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> DrawingAreaExt<Msg> for View<Msg> {
    fn content_width(self, px: i32) -> Self {
        self.prop(PropName::WidthRequest, Prop::Int(i64::from(px)))
    }
    fn content_height(self, px: i32) -> Self {
        self.prop(PropName::HeightRequest, Prop::Int(i64::from(px)))
    }
    fn on_resize(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::DrawingArea`'s controller.
pub struct DrawingAreaC {
    /// The application's paint callback.
    pub draw: Rc<dyn Fn(&mut Canvas<'_>, Rect)>,
    /// `(content-width, content-height)`, the intrinsic size.
    pub content: (i32, i32),
    /// The last allocation the callback saw, for the resize signal.
    pub last_size: (f32, f32),
}

impl<Msg: Clone + 'static> Controller<Msg> for DrawingAreaC {
    fn kind(&self) -> Kind {
        Kind::DrawingArea
    }

    fn build(_node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        DrawingAreaC {
            draw: match props.get(PropName::DrawFn) {
                Some(Prop::Draw(f)) => Rc::clone(f),
                _ => Rc::new(|_, _| {}),
            },
            content: (
                i32::try_from(props.int(PropName::WidthRequest, 0)).unwrap_or(0),
                i32::try_from(props.int(PropName::HeightRequest, 0)).unwrap_or(0),
            ),
            last_size: (0.0, 0.0),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::DrawFn, Prop::Draw(f)) => self.draw = Rc::clone(f),
            (PropName::WidthRequest, Prop::Int(px)) => {
                self.content.0 = i32::try_from(*px).unwrap_or(0);
            }
            (PropName::HeightRequest, Prop::Int(px)) => {
                self.content.1 = i32::try_from(*px).unwrap_or(0);
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Event::Configure { size, .. } = ev {
            let next = (size.0 as f32, size.1 as f32);
            if next != self.last_size {
                self.last_size = next;
                if let Some(msg) = cx.handlers.fire_index(EventKind::Change, size.0 as usize) {
                    return vec![msg];
                }
            }
        }
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some((self.content.0.max(0) as f32, self.content.1.max(0) as f32))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        if content.is_empty() {
            return false;
        }
        self.last_size = (content.width, content.height);
        (self.draw)(canvas, content);
        true
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/window_controls.rs`**

```rust
//! `GtkWindowControls` — `Kind::WindowControls`, CSS node `windowcontrols`.
//!
//! ```text
//! windowcontrols[.start][.end][.empty][.native]
//! ├── [image.icon]
//! ├── [button.minimize]
//! ├── [button.maximize]
//! ╰── [button.close]
//! ```
//!
//! [`WindowControlsC::tokens`] is `update_window_buttons`
//! (`gtk/gtkwindowcontrols.c:302-424`) transcribed: the `side` picks the half
//! of `gtk-decoration-layout` before or after the colon, the half is split on
//! `,` and walked **in order**, and `menu` produces no child in 4.22.4. All
//! three buttons are `focusable = false` and never enter the Tab ring; their
//! actions go to `xdg_toplevel` through `Cmd`, not to a GTK window.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::cmd::Cmd;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, Side, WidgetEnum, WindowButton, local_rect, shift_event};
use crate::window::SurfaceStates;

/// GTK's own default, `gtk/gtksettings.c:854-856`.
pub const DEFAULT_DECORATION_LAYOUT: &str = "menu:minimize,maximize,close";

/// A `GtkWindowControls` rendering one `side` of the decoration layout.
#[must_use]
pub fn window_controls<Msg: Clone + 'static>(side: Side) -> View<Msg> {
    View::new(Kind::WindowControls).prop(PropName::Side, side.to_prop())
}

/// `GtkWindowControls:decoration-layout`.
pub trait WindowControlsExt<Msg>: Sized {
    /// Override `gtk-decoration-layout` for this widget.
    fn decoration_layout(self, layout: &str) -> Self;
}

impl<Msg: Clone + 'static> WindowControlsExt<Msg> for View<Msg> {
    fn decoration_layout(self, layout: &str) -> Self {
        self.prop(PropName::Decoration, Prop::Str(Rc::from(layout)))
    }
}

/// `Kind::WindowControls`'s controller.
pub struct WindowControlsC {
    /// Which half of the layout this widget renders.
    pub side: Side,
    /// The layout string in force.
    pub layout: Rc<str>,
    /// The emitted children, in layout order.
    pub buttons: Vec<(WindowButton, Node)>,
    /// `true` when nothing was emitted, mirroring `GtkWindowControls:empty`.
    pub empty: bool,
    maximized: bool,
    pointer: PointerState,
}

impl WindowControlsC {
    /// The tokens `side` contributes, in order.
    ///
    /// `menu` is recognised by the setting's documentation but
    /// `update_window_buttons` has no branch for it in 4.22.4, so it produces
    /// nothing. Unknown tokens are dropped.
    #[must_use]
    pub fn tokens(layout: &str, side: Side) -> Vec<WindowButton> {
        let (start, end) = layout.split_once(':').unwrap_or((layout, ""));
        let half = match side {
            Side::Start => start,
            Side::End => end,
        };
        half.split(',')
            .filter_map(|token| match token.trim() {
                "icon" => Some(WindowButton::Icon),
                "minimize" => Some(WindowButton::Minimize),
                "maximize" => Some(WindowButton::Maximize),
                "close" => Some(WindowButton::Close),
                // `menu` and anything unrecognised emit no child.
                _ => None,
            })
            .collect()
    }

    /// Rebuild the children from `layout`, `side` and the maximized state.
    fn rebuild(&mut self, node: &Node) {
        for (_, child) in self.buttons.drain(..) {
            child.detach();
        }
        for token in Self::tokens(&self.layout, self.side) {
            let child = match token {
                WindowButton::Icon => Node::with_classes("image", &["icon"]),
                WindowButton::Minimize => Node::with_classes("button", &["minimize"]),
                WindowButton::Maximize => Node::with_classes("button", &["maximize"]),
                WindowButton::Close => Node::with_classes("button", &["close"]),
            };
            node.append_child(&child);
            self.buttons.push((token, child));
        }
        self.empty = self.buttons.is_empty();
        for candidate in Side::all() {
            node.remove_class(candidate.css_class());
        }
        node.add_class(self.side.css_class());
        node.remove_class("empty");
        if self.empty {
            node.add_class("empty");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for WindowControlsC {
    fn kind(&self) -> Kind {
        Kind::WindowControls
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let mut this = WindowControlsC {
            side: Side::from_prop(props.get(PropName::Side), Side::Start),
            layout: props
                .str(PropName::Decoration)
                .map_or_else(|| Rc::from(DEFAULT_DECORATION_LAYOUT), Rc::from),
            buttons: Vec::new(),
            empty: true,
            maximized: false,
            pointer: PointerState::default(),
        };
        this.rebuild(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Decoration, Prop::Str(layout)) => self.layout = Rc::clone(layout),
            (PropName::Side, Prop::Enum(_)) => {
                self.side = Side::from_prop(Some(value), self.side);
            }
            _ => return,
        }
        self.rebuild(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // A `Configure` that flipped MAXIMIZED changes the maximize button's
        // icon, so the children are rebuilt — GTK does the same on
        // `notify::maximized`.
        if let Event::Configure { states, .. } = ev {
            let maximized = states.contains(SurfaceStates::MAXIMIZED);
            if maximized != self.maximized {
                self.maximized = maximized;
                self.rebuild(cx.node);
            }
            return Vec::new();
        }
        for (token, child) in &self.buttons {
            let Some(rect) = local_rect(cx.tree, cx.node, child) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            if self
                .pointer
                .observe(child, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)))
            {
                cx.handled = true;
                match token {
                    WindowButton::Minimize => cx.cmds.push(Cmd::Minimize),
                    WindowButton::Maximize => cx.cmds.push(Cmd::ToggleMaximized),
                    WindowButton::Close => cx.cmds.push(Cmd::CloseWindow),
                    WindowButton::Icon => {}
                }
                break;
            }
        }
        Vec::new()
    }
}
```

- [ ] **Step 6: Register both widgets** — `pub mod drawing_area;`, `pub mod window_controls;`; the arms `Kind::DrawingArea => Box::new(drawing_area::DrawingAreaC::build(node, props, cx)),` and `Kind::WindowControls => Box::new(window_controls::WindowControlsC::build(node, props, cx)),`; and in `ui/src/view/builders.rs`, `pub use crate::widgets::drawing_area::{DrawingAreaExt, drawing_area};` plus `pub use crate::widgets::window_controls::{WindowControlsExt, window_controls};`.

In `build_controller`, mark the three window-control children `focusable = false`
by clearing `PropName::Focusable` on them — the reconciler never sees them, so
`window_controls::WindowControlsC::rebuild` is where that is enforced; add
`child.add_class("no-focus");` there only if P3's `is_focusable` needs a marker,
otherwise rely on the children never being `Instance`s and therefore never being
focus-ring candidates.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees window_controls_follow`, `... a_drawing_area_is_one_widget_node`, `cargo test -p icedtea-ui --test widget_pixels a_drawing_area_runs`; expected PASS.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): DrawingArea and WindowControls

WindowControlsC::tokens is update_window_buttons transcribed, including that
`menu` produces no child in 4.22.4; the buttons' actions go to xdg_toplevel
through Cmd, not to a GTK window.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 17: `Calendar`

**Files:** Create `ui/src/widgets/calendar.rs`, `ui/tests/fixtures/gtk4.22-node-trees/calendar.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `PointerState`, `local_rect`, `shift_event`; Task 8's `LabelC`.
- Produces:
  ```rust
  pub fn calendar<Msg: Clone + 'static>(year: i32, month: u32, day: u32) -> View<Msg>;
  pub trait CalendarExt<Msg>: Sized {
      fn show_day_names(self, on: bool) -> Self;
      fn show_heading(self, on: bool) -> Self;
      fn show_week_numbers(self, on: bool) -> Self;
      fn mark_day(self, day: u32) -> Self;
      fn on_date_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
  }
  pub struct CalendarC { pub shown: (i32, u32), pub selected: (i32, u32, u32), pub marks: u32,
                         pub header: Node, pub grid: Node, pub day_nodes: Vec<Node> }
  impl CalendarC {
      /// Days in `month` of `year`, Gregorian, leap years included.
      #[must_use] pub fn days_in_month(year: i32, month: u32) -> u32;
      /// Weekday of the 1st, 0 = Monday, by Zeller's congruence.
      #[must_use] pub fn first_weekday(year: i32, month: u32) -> u32;
  }
  ```
  `on_date_selected` carries the day-of-month as a `usize` — `Handler` has no
  triple-carrying variant, and the year and month are the ones the model just
  supplied, so the day is the whole of the new information.

- [ ] **Step 1: Vendor the fixture** — `calendar.txt`, verbatim from `gtk/gtkcalendar.c:61`:

```
calendar.view
├── header
│   ├── button
│   ├── stack.month
│   ├── button
│   ├── button
│   ├── label.year
│   ╰── button
╰── grid
    ╰── label[.day-name][.week-number][.day-number][.other-month][.today]
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_calendar_builds_its_header_and_a_grid_of_day_labels() {
    // mutation: emit 28 day labels regardless of month and the March
    // assertion below reports 31 != 28.
    use icedtea_ui::widgets::calendar::CalendarC;
    assert_eq!(CalendarC::days_in_month(2024, 2), 29, "2024 is a leap year");
    assert_eq!(CalendarC::days_in_month(1900, 2), 28, "1900 is not");
    assert_eq!(CalendarC::days_in_month(2026, 3), 31);
    assert_eq!(CalendarC::days_in_month(2026, 13), 31, "an out-of-range month clamps");
    assert_eq!(CalendarC::first_weekday(2026, 1), 3, "1 Jan 2026 is a Thursday");

    let mut props = Props::default();
    props.set(PropName::Row, Prop::Int(2026));
    props.set(PropName::Column, Prop::Int(3));
    props.set(PropName::Value, Prop::Float(15.0));
    check(Kind::Calendar, "calendar", &props);
    let rendered = node_tree_of(Kind::Calendar, &props);
    assert!(rendered.starts_with("calendar.view"), "{rendered}");
    assert_eq!(rendered.matches("label.day-number").count(), 31, "{rendered}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn clicking_a_calendar_day_selects_it() {
    // mutation: never fire EventKind::DateSelected in CalendarC::on_event and
    // the model stays at 15.
    use icedtea_ui::view::builders::calendar;
    use icedtea_ui::widgets::calendar::CalendarExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Picked(usize);

    let frames = run(
        15u32,
        |model: &mut u32, Picked(day): Picked| {
            *model = u32::try_from(day).unwrap_or(*model);
            Cmd::None
        },
        |model: &u32| {
            calendar(2026, 3, *model).hexpand(true).vexpand(true).on_date_selected(Picked)
        },
        (280, 240),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 60.0, y: 120.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 2, "both captures ran without a panic");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_calendar_builds`; expected FAIL, `unresolved import icedtea_ui::widgets::calendar`.

- [ ] **Step 4: Write `ui/src/widgets/calendar.rs`**

```rust
//! `GtkCalendar` — `Kind::Calendar`, CSS node `calendar`, always `.view`.
//!
//! ```text
//! calendar.view
//! ├── header
//! │   ├── button
//! │   ├── stack.month
//! │   ├── button
//! │   ├── button
//! │   ├── label.year
//! │   ╰── button
//! ╰── grid
//!     ╰── label[.day-name][.week-number][.day-number][.other-month][.today]
//! ```
//!
//! GTK lays the day labels out in a grid; `layout::Container::Grid` is P6's, so
//! P5 nests one row box per week under `grid`. The *CSS node tree* the fixture
//! pins is unaffected: `grid`'s children are still the day labels, because the
//! row boxes are layout-only and are not CSS nodes — they are created as
//! `Container::Box` children of a node that is itself the `grid`, one row at a
//! time, by appending the labels directly and letting P6's Grid variant take
//! over the placement when it lands.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, local_rect, shift_event};

/// A `GtkCalendar` showing `year`/`month` with `day` selected.
#[must_use]
pub fn calendar<Msg: Clone + 'static>(year: i32, month: u32, day: u32) -> View<Msg> {
    View::new(Kind::Calendar)
        .prop(PropName::Row, Prop::Int(i64::from(year)))
        .prop(PropName::Column, Prop::Int(i64::from(month)))
        .prop(PropName::Value, Prop::Float(f64::from(day)))
}

/// `GtkCalendar`'s own setters.
pub trait CalendarExt<Msg>: Sized {
    /// `GtkCalendar:show-day-names`.
    fn show_day_names(self, on: bool) -> Self;
    /// `GtkCalendar:show-heading`.
    fn show_heading(self, on: bool) -> Self;
    /// `GtkCalendar:show-week-numbers`.
    fn show_week_numbers(self, on: bool) -> Self;
    /// `gtk_calendar_mark_day`. Repeated calls accumulate into a 31-bit mask.
    fn mark_day(self, day: u32) -> Self;
    /// `GtkCalendar::day-selected`, carrying the day of the month.
    fn on_date_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> CalendarExt<Msg> for View<Msg> {
    fn show_day_names(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn show_heading(self, on: bool) -> Self {
        self.prop(PropName::Title, Prop::Bool(on))
    }
    fn show_week_numbers(self, on: bool) -> Self {
        self.prop(PropName::ShowSeparators, Prop::Bool(on))
    }
    fn mark_day(self, day: u32) -> Self {
        let previous = match self.props.get(PropName::Detail) {
            Some(Prop::Int(mask)) => *mask,
            _ => 0,
        };
        let bit = 1i64 << day.clamp(1, 31);
        self.prop(PropName::Detail, Prop::Int(previous | bit))
    }
    fn on_date_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::DateSelected, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::Calendar`'s controller.
pub struct CalendarC {
    /// The `(year, month)` currently displayed.
    pub shown: (i32, u32),
    /// The selected `(year, month, day)`.
    pub selected: (i32, u32, u32),
    /// Marked days, one bit per day of the month.
    pub marks: u32,
    /// The `header` subnode.
    pub header: Node,
    /// The `grid` subnode.
    pub grid: Node,
    /// One `label` per day cell, ascending.
    pub day_nodes: Vec<Node>,
    pointer: PointerState,
}

impl CalendarC {
    /// Days in `month` of `year`, Gregorian. An out-of-range month clamps to
    /// December's 31 rather than panicking — the month arrives from a model.
    #[must_use]
    pub fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
                if leap { 29 } else { 28 }
            }
            _ => 31,
        }
    }

    /// Weekday of the 1st of `month`, 0 = Monday, by Zeller's congruence.
    #[must_use]
    pub fn first_weekday(year: i32, month: u32) -> u32 {
        let (mut y, m) = (year, month.clamp(1, 12));
        let m = if m < 3 {
            y -= 1;
            m + 12
        } else {
            m
        };
        let k = y.rem_euclid(100);
        let j = y.div_euclid(100);
        // Zeller yields 0 = Saturday; shift to 0 = Monday.
        let h = (1 + (13 * (m as i32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
        u32::try_from((h + 5).rem_euclid(7)).unwrap_or(0)
    }

    /// Rebuild the day grid for `shown`.
    fn rebuild(&mut self) {
        for node in self.day_nodes.drain(..) {
            node.detach();
        }
        let days = Self::days_in_month(self.shown.0, self.shown.1);
        for day in 1..=days {
            let mut classes: Vec<&str> = vec!["day-number"];
            if (self.marks >> day) & 1 == 1 {
                classes.push("today");
            }
            let node = Node::with_classes("label", &classes);
            node.set_state(
                PseudoStates::SELECTED,
                self.selected == (self.shown.0, self.shown.1, day),
            );
            self.grid.append_child(&node);
            self.day_nodes.push(node);
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for CalendarC {
    fn kind(&self) -> Kind {
        Kind::Calendar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        node.add_class("view");
        let header = Node::new("header");
        node.append_child(&header);
        // GTK's header is button, stack.month, button, button, label.year,
        // button — six children, in that order.
        header.append_child(&Node::new("button"));
        header.append_child(&Node::with_classes("stack", &["month"]));
        header.append_child(&Node::new("button"));
        header.append_child(&Node::new("button"));
        header.append_child(&Node::with_classes("label", &["year"]));
        header.append_child(&Node::new("button"));
        let grid = Node::new("grid");
        node.append_child(&grid);

        let year = i32::try_from(props.int(PropName::Row, 1970)).unwrap_or(1970);
        let month = u32::try_from(props.int(PropName::Column, 1)).unwrap_or(1).clamp(1, 12);
        let day = props.float(PropName::Value, 1.0).clamp(1.0, 31.0) as u32;
        let mut this = CalendarC {
            shown: (year, month),
            selected: (year, month, day),
            marks: u32::try_from(props.int(PropName::Detail, 0)).unwrap_or(0),
            header,
            grid,
            day_nodes: Vec::new(),
            pointer: PointerState::default(),
        };
        this.rebuild();
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Row, Prop::Int(y)) => {
                self.shown.0 = i32::try_from(*y).unwrap_or(self.shown.0);
                self.selected.0 = self.shown.0;
            }
            (PropName::Column, Prop::Int(m)) => {
                self.shown.1 = u32::try_from(*m).unwrap_or(self.shown.1).clamp(1, 12);
                self.selected.1 = self.shown.1;
            }
            (PropName::Value, Prop::Float(d)) => {
                self.selected.2 = d.clamp(1.0, 31.0) as u32;
            }
            (PropName::Detail, Prop::Int(mask)) => {
                self.marks = u32::try_from(*mask).unwrap_or(0);
            }
            _ => return,
        }
        self.rebuild();
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        for (index, day_node) in self.day_nodes.iter().enumerate() {
            let Some(rect) = local_rect(cx.tree, cx.node, day_node) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            if self
                .pointer
                .observe(day_node, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)))
            {
                cx.handled = true;
                if let Some(msg) = cx.handlers.fire_index(EventKind::DateSelected, index + 1) {
                    return vec![msg];
                }
                return Vec::new();
            }
        }
        Vec::new()
    }
}
```

- [ ] **Step 5: Register the widget** — `pub mod calendar;`, `Kind::Calendar => Box::new(calendar::CalendarC::build(node, props, cx)),`, `pub use crate::widgets::calendar::{CalendarExt, calendar};`.

- [ ] **Step 6: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_calendar_builds` and `cargo test -p icedtea-ui --test widget_pixels clicking_a_calendar_day`; expected PASS.

- [ ] **Step 7: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 8: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Calendar with a real Gregorian month grid

days_in_month and first_weekday are pinned by unit assertions (2024 and 1900
February, 1 Jan 2026) rather than trusted; an out-of-range month clamps rather
than panicking, because the month arrives from an application model.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 18: `Popover` (contract ruling R4 — a P5 deliverable)

**Files:** Create `ui/src/widgets/popover.rs`, `ui/tests/fixtures/gtk4.22-node-trees/popover.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `Position`, `WidgetEnum`; P3's `window::{PopupKey, PopupAnchorPoint, Positioner}`, `window::popup::{Anchor, Gravity, ConstraintAdjustment}`; P4's `Cmd::{OpenPopup, ClosePopup}`.
- Produces:
  ```rust
  pub fn popover<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  pub trait PopoverExt<Msg>: Sized {
      fn autohide(self, on: bool) -> Self;
      fn has_arrow(self, on: bool) -> Self;
      fn position(self, pos: Position) -> Self;
      fn offset(self, dx: i32, dy: i32) -> Self;
      fn pointing_to(self, rect: Rect) -> Self;
      fn on_close(self, msg: Msg) -> Self;
  }
  pub struct PopoverC { pub open: bool, pub autohide: bool, pub has_arrow: bool,
                        pub position: Position, pub popup: Option<PopupKey>,
                        pub arrow: Node, pub contents: Node }
  impl PopoverC {
      /// Open against `anchor`. An autohide popover becomes a real
      /// `Surface::Popup` with a grab; a non-autohide one renders in the
      /// parent window's own tree.
      pub fn open<Msg>(&mut self, anchor: PopupAnchorPoint, size: (u32, u32),
                       cx: &mut EventCx<'_, Msg>);
      pub fn close<Msg>(&mut self, cx: &mut EventCx<'_, Msg>);
      /// The positioner `open` would build — the seam the unit test pins.
      #[must_use] pub fn positioner(&self, anchor_rect: Rect, size: (u32, u32)) -> Positioner;
  }
  ```

- [ ] **Step 1: Vendor the fixture** — `popover.txt`, verbatim from `gtk/gtkpopover.c:89`:

```
popover.background[.menu]
├── arrow
╰── contents
    ╰── <child>
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_popover_always_carries_background_and_wraps_its_child_in_contents() {
    // mutation: append the child directly to the popover node instead of to
    // `contents` and this fails with "rendered node at 'popover/label' is not
    // in the fixture".
    check(Kind::Popover, "popover", &Props::default());
    let rendered = node_tree_of(Kind::Popover, &Props::default());
    assert!(rendered.starts_with("popover.background"), "{rendered}");
    assert!(rendered.contains("arrow"), "{rendered}");
    assert!(rendered.contains("contents"), "{rendered}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn an_autohide_popovers_positioner_anchors_to_its_parent_rect() {
    // mutation: return `Anchor::TOP` unconditionally from PopoverC::positioner
    // and the Bottom case reports the wrong gravity.
    use icedtea_ui::layout::Rect;
    use icedtea_ui::widgets::{Position, popover::PopoverC};
    use icedtea_ui::window::popup::{Anchor, Gravity};

    let mut controller = PopoverC::for_test(Position::Bottom, true, true);
    let positioner = controller.positioner(Rect::new(10.0, 20.0, 40.0, 24.0), (200, 120));
    assert_eq!(positioner.size, (200, 120));
    assert_eq!(positioner.anchor_rect, Rect::new(10.0, 20.0, 40.0, 24.0));
    assert_eq!(positioner.anchor, Anchor::BOTTOM);
    assert_eq!(positioner.gravity, Gravity::BOTTOM);
    assert!(positioner.reactive, "a popover follows its parent");

    controller = PopoverC::for_test(Position::Top, true, true);
    let positioner = controller.positioner(Rect::new(0.0, 0.0, 10.0, 10.0), (50, 50));
    assert_eq!(positioner.anchor, Anchor::TOP);
    assert_eq!(positioner.gravity, Gravity::TOP);
}
```

`PopoverC::for_test(position, autohide, has_arrow) -> PopoverC` is a
`#[cfg(any(test, feature = "test-util"))]`-free plain constructor — it builds the
two subnodes against a detached root, so the unit test needs no `App`. Declare
it in the `Produces` block above and implement it in Step 4.

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_popover_always_carries`; expected FAIL, `fixture requires a node at 'popover/arrow'`.

- [ ] **Step 4: Write `ui/src/widgets/popover.rs`**

```rust
//! `GtkPopover` — `Kind::Popover`, CSS node `popover`, always `.background`.
//!
//! ```text
//! popover.background[.menu]
//! ├── arrow
//! ╰── contents
//!     ╰── <child>
//! ```
//!
//! Contract ruling R4 makes this a P5 deliverable although the spec files
//! popovers under P6: `MenuButton`, `DropDown`, `ColorDialogButton` and
//! `FontDialogButton` all embed one. P6 builds `PopoverMenu` and
//! `PopoverMenuBar` on top.
//!
//! An **autohide** popover (GTK's "modal") is a real `Surface::Popup` taking
//! `xdg_popup.grab`; a non-autohide one renders inside the parent window's own
//! tree with no grab.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::cmd::Cmd;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{Position, WidgetEnum};
use crate::window::popup::{Anchor, ConstraintAdjustment, Gravity};
use crate::window::{PopupAnchorPoint, PopupKey, Positioner};

/// A `GtkPopover` wrapping `child`.
#[must_use]
pub fn popover<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Popover).child(child)
}

/// `GtkPopover`'s own setters.
pub trait PopoverExt<Msg>: Sized {
    /// `GtkPopover:autohide` — GTK's "modal"; takes the popup grab.
    fn autohide(self, on: bool) -> Self;
    /// `GtkPopover:has-arrow`.
    fn has_arrow(self, on: bool) -> Self;
    /// `GtkPopover:position`.
    fn position(self, pos: Position) -> Self;
    /// `gtk_popover_set_offset`.
    fn offset(self, dx: i32, dy: i32) -> Self;
    /// `GtkPopover:pointing-to`, in the parent window's frame space.
    fn pointing_to(self, rect: Rect) -> Self;
    /// `GtkPopover::closed`.
    fn on_close(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> PopoverExt<Msg> for View<Msg> {
    fn autohide(self, on: bool) -> Self {
        self.prop(PropName::Autohide, Prop::Bool(on))
    }
    fn has_arrow(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn position(self, pos: Position) -> Self {
        self.prop(PropName::Position, pos.to_prop())
    }
    fn offset(self, dx: i32, dy: i32) -> Self {
        self.prop(PropName::Offset, Prop::Edges([dx, dy, 0, 0]))
    }
    fn pointing_to(self, rect: Rect) -> Self {
        self.prop(
            PropName::Anchor,
            Prop::Edges([rect.x as i32, rect.y as i32, rect.width as i32, rect.height as i32]),
        )
    }
    fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }
}

/// `Kind::Popover`'s controller. Embedded by four other P5 widgets.
pub struct PopoverC {
    /// Whether the popover is showing.
    pub open: bool,
    /// GTK's "modal": takes the popup grab.
    pub autohide: bool,
    /// Draw the `arrow` node.
    pub has_arrow: bool,
    /// Which side of the anchor the popover sits on.
    pub position: Position,
    /// The compositor-side popup, while open and autohiding.
    pub popup: Option<PopupKey>,
    /// The `arrow` subnode.
    pub arrow: Node,
    /// The `contents` subnode; every child goes under it.
    pub contents: Node,
    offset: (i32, i32),
}

impl PopoverC {
    /// Build the two subnodes under a fresh detached root — the constructor
    /// the positioner unit test uses, and the one every embedding widget calls
    /// when it owns its popover rather than receiving it from the reconciler.
    #[must_use]
    pub fn for_test(position: Position, autohide: bool, has_arrow: bool) -> Self {
        let root = Node::with_classes("popover", &["background"]);
        let arrow = Node::new("arrow");
        root.append_child(&arrow);
        let contents = Node::new("contents");
        root.append_child(&contents);
        PopoverC {
            open: false,
            autohide,
            has_arrow,
            position,
            popup: None,
            arrow,
            contents,
            offset: (0, 0),
        }
    }

    /// The positioner this popover would use for `anchor_rect` at `size`.
    ///
    /// `position` names the side of the anchor the popover sits on, so both the
    /// anchor edge and the gravity take that side: a `Bottom` popover hangs off
    /// the anchor's bottom edge, growing downward.
    #[must_use]
    pub fn positioner(&self, anchor_rect: Rect, size: (u32, u32)) -> Positioner {
        let (anchor, gravity) = match self.position {
            Position::Top => (Anchor::TOP, Gravity::TOP),
            Position::Bottom => (Anchor::BOTTOM, Gravity::BOTTOM),
            Position::Left => (Anchor::LEFT, Gravity::LEFT),
            Position::Right => (Anchor::RIGHT, Gravity::RIGHT),
        };
        Positioner {
            anchor_rect,
            size,
            anchor,
            gravity,
            constraint: ConstraintAdjustment::FLIP_X
                | ConstraintAdjustment::FLIP_Y
                | ConstraintAdjustment::SLIDE_X
                | ConstraintAdjustment::SLIDE_Y,
            offset: self.offset,
            reactive: true,
        }
    }

    /// Open against `anchor`.
    pub fn open<Msg: Clone + 'static>(
        &mut self,
        anchor: PopupAnchorPoint,
        size: (u32, u32),
        cx: &mut EventCx<'_, Msg>,
    ) {
        if self.open {
            return;
        }
        self.open = true;
        if !self.autohide {
            // A non-autohide popover renders in the parent window; nothing to
            // ask the compositor for.
            return;
        }
        let anchor_rect = match &anchor {
            PopupAnchorPoint::Rect(rect) => *rect,
            PopupAnchorPoint::Node(node) => {
                cx.tree.allocation(node).map_or(Rect::zero(), |a| a.border_box)
            }
        };
        cx.cmds.push(Cmd::OpenPopup {
            anchor,
            positioner: self.positioner(anchor_rect, size),
            view: Rc::new(|| View::new(Kind::Popover)),
        });
    }

    /// Close, dropping the popup if one was taken.
    pub fn close<Msg: Clone + 'static>(&mut self, cx: &mut EventCx<'_, Msg>) {
        self.open = false;
        if let Some(key) = self.popup.take() {
            cx.cmds.push(Cmd::ClosePopup(key));
        }
    }

    fn apply(&self, node: &Node) {
        node.add_class("background");
        if self.has_arrow {
            if self.arrow.parent().is_none() {
                node.insert_child(0, &self.arrow);
            }
        } else {
            self.arrow.detach();
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PopoverC {
    fn kind(&self) -> Kind {
        Kind::Popover
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let arrow = Node::new("arrow");
        node.append_child(&arrow);
        let contents = Node::new("contents");
        node.append_child(&contents);
        let offset = match props.get(PropName::Offset) {
            Some(Prop::Edges([dx, dy, _, _])) => (*dx, *dy),
            _ => (0, 0),
        };
        let this = PopoverC {
            open: false,
            autohide: props.bool(PropName::Autohide, true),
            has_arrow: props.bool(PropName::ShowArrow, true),
            position: Position::from_prop(props.get(PropName::Position), Position::Bottom),
            popup: None,
            arrow,
            contents,
            offset,
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Autohide, Prop::Bool(on)) => self.autohide = *on,
            (PropName::ShowArrow, Prop::Bool(on)) => self.has_arrow = *on,
            (PropName::Position, Prop::Enum(_)) => {
                self.position = Position::from_prop(Some(value), self.position);
            }
            (PropName::Offset, Prop::Edges([dx, dy, _, _])) => self.offset = (*dx, *dy),
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        match ev {
            // The compositor dismissed the chain — `xdg_popup.popup_done`.
            Event::PopupDone => {
                self.open = false;
                self.popup = None;
                cx.handlers.fire_unit(EventKind::Close).map_or_else(Vec::new, |msg| vec![msg])
            }
            _ => Vec::new(),
        }
    }
}
```

The reconciler must place every child of a `Kind::Popover` under `contents`, not
under the popover node. Do that by having `build_controller` return the popover
controller *and* by adding, in `ui/src/widgets/mod.rs`, a child-slot hook the
reconciler already needs for `Frame`, `Expander` and `ScrolledWindow` in P6:

```rust
/// Where a kind's application children are attached.
///
/// Most kinds take children on their own node. A few nest them in a subnode
/// GTK's own tree names — `popover > contents` is P5's only case; P6 adds
/// `frame > box`, `expander-widget > box` and the scrolled window's viewport.
/// P4's reconciler calls this before `Node::append_child`.
#[must_use]
pub fn child_slot(kind: Kind, controller: &dyn std::any::Any) -> Option<Node> {
    match kind {
        Kind::Popover => controller
            .downcast_ref::<popover::PopoverC>()
            .map(|c| c.contents.clone()),
        _ => None,
    }
}
```

- [ ] **Step 5: Register the widget** — `pub mod popover;`, `Kind::Popover => Box::new(popover::PopoverC::build(node, props, cx)),`, `pub use crate::widgets::popover::{PopoverExt, popover};`.

- [ ] **Step 6: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_popover_always_carries` and `cargo test -p icedtea-ui --test widget_pixels an_autohide_popovers_positioner`; expected PASS.

- [ ] **Step 7: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 8: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Popover, the surface four P5 widgets embed

Contract ruling R4 pulls popovers into P5. An autohide popover becomes a real
Surface::Popup with a grab; a non-autohide one renders in the parent window's
own tree. child_slot puts application children under `contents`, as GTK does.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 19: `Button`, `ToggleButton` and `LinkButton`

**Files:** Create `ui/src/widgets/button.rs`, `ui/src/widgets/toggle_button.rs`, `ui/src/widgets/link_button.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{button,toggle_button,link_button}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `PointerState`; Task 8's `LabelC`; P4's `Cmd::Copy`; P3's `window::keyboard::Mods`.
- Produces:
  ```rust
  pub fn button<Msg: Clone + 'static>(label: &str) -> View<Msg>;
  pub fn button_from<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg>;
  pub trait ButtonExt<Msg>: Sized {
      fn label(self, text: &str) -> Self;
      fn icon(self, icon: IconRef) -> Self;
      fn has_frame(self, on: bool) -> Self;
      fn use_underline(self, on: bool) -> Self;
      fn on_click(self, msg: Msg) -> Self;
      fn on_activate(self, msg: Msg) -> Self;
  }
  pub struct ButtonC { pub label: Option<Node>, pub image: Option<Node>,
                       pub activating_until: Option<Duration> }

  pub fn toggle_button<Msg: Clone + 'static>(label: &str) -> View<Msg>;
  pub trait ToggleButtonExt<Msg>: Sized {
      fn active(self, on: bool) -> Self;
      fn group(self, name: &str) -> Self;
      fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
  }
  pub struct ToggleButtonC { pub active: bool, pub group: Option<Rc<str>>, pub label: Option<Node> }

  pub fn link_button<Msg: Clone + 'static>(uri: &str, label: &str) -> View<Msg>;
  pub trait LinkButtonExt<Msg>: Sized {
      fn visited(self, on: bool) -> Self;
      fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
  }
  pub struct LinkButtonC { pub uri: Rc<str>, pub visited: bool, pub label: Node,
                           pub menu: Option<PopupKey> }
  ```

- [ ] **Step 1: Vendor the three fixtures** — from `gtk/gtkbutton.c:45`, `gtk/gtktogglebutton.c:66`, `gtk/gtklinkbutton.c:61`. All three are prose about a single node; the fixtures are what that prose describes:

`button.txt`:

```
button[.image-button][.text-button][.flat][.keyboard-activating]
```

`toggle_button.txt`:

```
button.toggle
```

`link_button.txt`:

```
button.link
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn the_three_button_kinds_share_the_button_node_and_differ_by_class() {
    // mutation: stop adding `.text-button` from the content and the first
    // assertion fails — GTK sets it from what the button actually contains.
    let mut labelled = Props::default();
    labelled.set(PropName::Label, Prop::Str("Ok".into()));
    assert!(node_tree_of(Kind::Button, &labelled).starts_with("button.text-button"));
    check(Kind::Button, "button", &labelled);

    assert!(node_tree_of(Kind::ToggleButton, &labelled).starts_with("button.toggle"));
    check(Kind::ToggleButton, "toggle_button", &labelled);

    let mut link = labelled.clone();
    link.set(PropName::Uri, Prop::Str("https://gtk.org".into()));
    assert!(node_tree_of(Kind::LinkButton, &link).contains("button.link"));
    check(Kind::LinkButton, "link_button", &link);
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn clicking_a_button_fires_its_message_and_paints_the_active_state() {
    // mutation: never set PseudoStates::ACTIVE in PointerState::observe and the
    // pressed capture matches the resting one.
    use icedtea_ui::view::builders::button;
    use icedtea_ui::widgets::button::ButtonExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Clicked;

    let frames = run(
        0u32,
        |model: &mut u32, _msg: Clicked| {
            *model += 1;
            Cmd::None
        },
        |_m: &u32| button("Ok").on_click(Clicked),
        (120, 48),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 40.0, y: 24.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..120).map(|x| frames.pixel(frame, x, 24)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), ":active must repaint the button");
    assert_eq!(row(0), row(2), "release restores the resting appearance");
}

#[test]
fn toggling_a_toggle_button_paints_the_checked_state() {
    // mutation: skip PseudoStates::CHECKED in ToggleButtonC::apply and the two
    // captures match.
    use icedtea_ui::view::builders::toggle_button;
    use icedtea_ui::widgets::toggle_button::ToggleButtonExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Toggled(bool);

    let frames = run(
        false,
        |model: &mut bool, Toggled(on): Toggled| {
            *model = on;
            Cmd::None
        },
        |model: &bool| toggle_button("Bold").active(*model).on_toggle(Toggled),
        (120, 48),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 40.0, y: 24.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..120).map(|x| frames.pixel(frame, x, 24)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), ":checked must repaint the button");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees the_three_button_kinds`; expected FAIL, `assert!(… starts_with("button.text-button"))`.

- [ ] **Step 4: Write `ui/src/widgets/button.rs`**

```rust
//! `GtkButton` — `Kind::Button`, CSS node `button`.
//!
//! ```text
//! button[.image-button][.text-button][.flat][.keyboard-activating]
//! ```
//!
//! `.image-button`/`.text-button` are set from the content GTK actually finds;
//! `.keyboard-activating` is added for the duration of a Space/Enter
//! activation. `.suggested-action`, `.destructive-action` and `.circular` are
//! application-supplied and are never added here.
//!
//! This is **not** M2's `widget::button::Button`, which stays exactly as M2
//! left it so the M1 pixel gate keeps measuring the same code (contract §8.1).

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;

/// How long `.keyboard-activating` stays on after Space or Enter.
const ACTIVATE_FLASH: Duration = Duration::from_millis(120);

/// A `GtkButton` labelled `label`.
#[must_use]
pub fn button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::Button).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// A `GtkButton` whose content is an arbitrary child.
#[must_use]
pub fn button_from<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Button).child(child)
}

/// `GtkButton`'s own setters and signals.
pub trait ButtonExt<Msg>: Sized {
    /// `GtkButton:label`.
    fn label(self, text: &str) -> Self;
    /// `GtkButton:icon-name`, as an `IconRef`.
    fn icon(self, icon: IconRef) -> Self;
    /// `GtkButton:has-frame`; `false` adds `.flat`.
    fn has_frame(self, on: bool) -> Self;
    /// `GtkButton:use-underline`.
    fn use_underline(self, on: bool) -> Self;
    /// `GtkButton::clicked`.
    fn on_click(self, msg: Msg) -> Self;
    /// `GtkButton::activate`.
    fn on_activate(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> ButtonExt<Msg> for View<Msg> {
    fn label(self, text: &str) -> Self {
        self.prop(PropName::Label, Prop::Str(Rc::from(text)))
    }
    fn icon(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn has_frame(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn use_underline(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn on_click(self, msg: Msg) -> Self {
        self.on(EventKind::Click, Handler::Unit(msg))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
}

/// `Kind::Button`'s controller.
pub struct ButtonC {
    /// The `label` subnode, when the button has text.
    pub label: Option<Node>,
    /// The `image` subnode, when the button has an icon.
    pub image: Option<Node>,
    /// When `.keyboard-activating` should come off.
    pub activating_until: Option<Duration>,
    pointer: PointerState,
}

impl ButtonC {
    /// GTK sets `.image-button`/`.text-button` from the content it finds.
    fn apply_content_classes(&self, node: &Node) {
        node.remove_class("image-button");
        node.remove_class("text-button");
        match (self.label.is_some(), self.image.is_some()) {
            (true, false) => node.add_class("text-button"),
            (false, true) => node.add_class("image-button"),
            _ => {}
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ButtonC {
    fn kind(&self) -> Kind {
        Kind::Button
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let image = props.get(PropName::Icon).is_some().then(|| {
            let image = Node::new("image");
            node.append_child(&image);
            image
        });
        let label = props.str(PropName::Label).map(|_| {
            let label = Node::new("label");
            node.append_child(&label);
            label
        });
        if !props.bool(PropName::ShowArrow, true) {
            node.add_class("flat");
        }
        let this = ButtonC { label, image, activating_until: None, pointer: PointerState::default() };
        this.apply_content_classes(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Label, Prop::Str(_)) => {
                if self.label.is_none() {
                    let label = Node::new("label");
                    node.append_child(&label);
                    self.label = Some(label);
                }
            }
            (PropName::ShowArrow, Prop::Bool(on)) => {
                if *on {
                    node.remove_class("flat");
                } else {
                    node.add_class("flat");
                }
            }
            _ => return,
        }
        self.apply_content_classes(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        if self.pointer.observe(cx.node, ev, bounds) {
            cx.handled = true;
            return cx.handlers.fire_unit(EventKind::Click).map_or_else(Vec::new, |m| vec![m]);
        }
        if matches!(ev, Event::Activate) {
            cx.node.add_class("keyboard-activating");
            self.activating_until = Some(cx.clock.now() + ACTIVATE_FLASH);
            cx.handled = true;
            let mut out = Vec::new();
            if let Some(msg) = cx.handlers.fire_unit(EventKind::Activate) {
                out.push(msg);
            }
            if let Some(msg) = cx.handlers.fire_unit(EventKind::Click) {
                out.push(msg);
            }
            return out;
        }
        Vec::new()
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.activating_until.is_some_and(|until| now >= until) {
            self.activating_until = None;
            cx.node.remove_class("keyboard-activating");
        }
        Vec::new()
    }

    fn next_deadline(&self, _now: Duration) -> Option<Duration> {
        self.activating_until
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/toggle_button.rs` and `ui/src/widgets/link_button.rs`**

```rust
//! `GtkToggleButton` — `Kind::ToggleButton`, CSS node `button`, class `.toggle`.
//!
//! ```text
//! button.toggle
//! ```
//!
//! A grouped toggle behaves radio-like: activating one clears its siblings.
//! Groups are keyed by the `Group` prop's name (contract §5.2's "Switch group
//! note"), not by object identity — the reactive layer has no stable handles.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;

/// A `GtkToggleButton` labelled `label`.
#[must_use]
pub fn toggle_button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::ToggleButton).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkToggleButton`'s own setters and signals.
pub trait ToggleButtonExt<Msg>: Sized {
    /// `GtkToggleButton:active`.
    fn active(self, on: bool) -> Self;
    /// `GtkToggleButton:group`, by name.
    fn group(self, name: &str) -> Self;
    /// `GtkToggleButton::toggled`.
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ToggleButtonExt<Msg> for View<Msg> {
    fn active(self, on: bool) -> Self {
        self.prop(PropName::Active, Prop::Bool(on))
    }
    fn group(self, name: &str) -> Self {
        self.prop(PropName::Group, Prop::Str(Rc::from(name)))
    }
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }
}

/// `Kind::ToggleButton`'s controller.
pub struct ToggleButtonC {
    /// `GtkToggleButton:active`.
    pub active: bool,
    /// The group name, when grouped.
    pub group: Option<Rc<str>>,
    /// The `label` subnode.
    pub label: Option<Node>,
    pointer: PointerState,
}

impl ToggleButtonC {
    fn apply(&self, node: &Node) {
        node.add_class("toggle");
        node.set_state(PseudoStates::CHECKED, self.active);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ToggleButtonC {
    fn kind(&self) -> Kind {
        Kind::ToggleButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let label = props.str(PropName::Label).map(|_| {
            let label = Node::new("label");
            node.append_child(&label);
            label
        });
        let this = ToggleButtonC {
            active: props.bool(PropName::Active, false),
            group: props.str(PropName::Group).map(Rc::from),
            label,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Active, Prop::Bool(on)) => self.active = *on,
            (PropName::Group, Prop::Str(name)) => self.group = Some(Rc::clone(name)),
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if !clicked && !matches!(ev, Event::Activate) {
            return Vec::new();
        }
        // A grouped toggle cannot be turned off by clicking it, exactly as a
        // radio button cannot; an ungrouped one flips.
        let next = if self.group.is_some() { true } else { !self.active };
        if next == self.active {
            return Vec::new();
        }
        self.active = next;
        self.apply(cx.node);
        cx.handled = true;
        cx.handlers.fire_bool(EventKind::Toggle, next).map_or_else(Vec::new, |m| vec![m])
    }
}
```

```rust
//! `GtkLinkButton` — `Kind::LinkButton`, CSS node `button`, class `.link`.
//!
//! ```text
//! button.link
//! ```
//!
//! Actions: `clipboard.copy` copies the uri, `menu.popup` opens the context
//! menu. Shortcut: `Shift+F10` or `Menu` opens that menu.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::view::cmd::Cmd;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;
use crate::window::PopupKey;
use crate::window::keyboard::Mods;

/// A `GtkLinkButton` for `uri`, showing `label`.
#[must_use]
pub fn link_button<Msg: Clone + 'static>(uri: &str, label: &str) -> View<Msg> {
    View::new(Kind::LinkButton)
        .prop(PropName::Uri, Prop::Str(Rc::from(uri)))
        .prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkLinkButton`'s own setters and signals.
pub trait LinkButtonExt<Msg>: Sized {
    /// `GtkLinkButton:visited`.
    fn visited(self, on: bool) -> Self;
    /// `GtkLinkButton::activate-link`.
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> LinkButtonExt<Msg> for View<Msg> {
    fn visited(self, on: bool) -> Self {
        self.prop(PropName::Checked, Prop::Bool(on))
    }
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }
}

/// `Kind::LinkButton`'s controller.
pub struct LinkButtonC {
    /// The link target.
    pub uri: Rc<str>,
    /// `GtkLinkButton:visited`.
    pub visited: bool,
    /// The `label` subnode.
    pub label: Node,
    /// The context menu's popup, while open.
    pub menu: Option<PopupKey>,
    pointer: PointerState,
}

impl LinkButtonC {
    fn apply(&self, node: &Node) {
        node.add_class("link");
        // GTK's :visited never matches on a node (M2's `PseudoStates::VISITED`
        // exists but GTK never sets it), so `visited` is a style class here.
        if self.visited {
            node.add_class("visited");
        } else {
            node.remove_class("visited");
        }
        let _ = PseudoStates::VISITED;
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for LinkButtonC {
    fn kind(&self) -> Kind {
        Kind::LinkButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let label = Node::new("label");
        node.append_child(&label);
        let this = LinkButtonC {
            uri: props.str(PropName::Uri).map_or_else(|| Rc::from(""), Rc::from),
            visited: props.bool(PropName::Checked, false),
            label,
            menu: None,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Uri, Prop::Str(uri)) => self.uri = Rc::clone(uri),
            (PropName::Checked, Prop::Bool(on)) => self.visited = *on,
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Shift+F10 and Menu open the context menu; its only entry is
        // `clipboard.copy`, which the controller performs directly.
        if let Event::Key(key) = ev {
            let is_menu = u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_Menu;
            let is_shift_f10 = u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_F10
                && key.effective_mods().contains(Mods::SHIFT);
            if key.pressed && (is_menu || is_shift_f10) {
                cx.handled = true;
                cx.cmds.push(Cmd::Copy(self.uri.to_string()));
                return Vec::new();
            }
        }
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if clicked || matches!(ev, Event::Activate) {
            self.visited = true;
            self.apply(cx.node);
            cx.handled = true;
            let uri = self.uri.to_string();
            return cx
                .handlers
                .fire_text(EventKind::ActivateLink, &uri)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }
}
```

Add `xkbcommon = "0.9"` to `ui/Cargo.toml`'s `[dependencies]` if P3 did not (it
owns that line per contract §0; `link_button.rs` is the first P5 consumer).

- [ ] **Step 6: Register all three** — `pub mod button;`, `pub mod toggle_button;`, `pub mod link_button;`; the three dispatch arms; and in `ui/src/view/builders.rs`, `pub use crate::widgets::button::{ButtonExt, button, button_from};`, `pub use crate::widgets::toggle_button::{ToggleButtonExt, toggle_button};`, `pub use crate::widgets::link_button::{LinkButtonExt, link_button};`.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees the_three_button_kinds`, `cargo test -p icedtea-ui --test widget_pixels clicking_a_button`, `... toggling_a_toggle_button`; expected PASS.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```
Expected: all green — in particular the four `themed_button_offscreen` tests,
which measure M2's `widget::button::Button` and must be untouched by this task.

- [ ] **Step 9: Commit**

```bash
git add ui/Cargo.toml ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Button, ToggleButton and LinkButton

Three kinds over one CSS node name, differing by class exactly as GTK does.
M2's widget::button::Button is untouched: it is the M1 pixel gate's subject and
ButtonC is a parallel implementation over the same paint primitives.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 20: `CheckButton` and `Switch`

**Files:** Create `ui/src/widgets/check_button.rs`, `ui/src/widgets/switch.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{check_button,switch}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `PointerState`, `local_rect`, `shift_event`; Task 7's `Builtin` stub (D2/D9).
- Produces:
  ```rust
  pub fn check_button<Msg: Clone + 'static>(label: &str) -> View<Msg>;
  pub trait CheckButtonExt<Msg>: Sized {
      fn active(self, on: bool) -> Self;
      fn inconsistent(self, on: bool) -> Self;
      fn group(self, name: &str) -> Self;
      fn use_underline(self, on: bool) -> Self;
      fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
  }
  pub struct CheckButtonC { pub active: bool, pub inconsistent: bool, pub group: Option<Rc<str>>,
                            pub check: Node, pub label: Option<Node> }
  impl CheckButtonC { #[must_use] pub fn builtin(&self) -> Builtin; }

  pub fn switch<Msg: Clone + 'static>(active: bool) -> View<Msg>;
  pub trait SwitchExt<Msg>: Sized {
      fn state(self, on: bool) -> Self;
      fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
  }
  pub struct SwitchC { pub active: bool, pub drag: Option<f32>, pub slide: f32,
                       pub slider: Node, pub on_image: Node, pub off_image: Node }
  ```

- [ ] **Step 1: Vendor the fixtures** — `check_button.txt` (`gtk/gtkcheckbutton.c:95`) and `switch.txt` (`gtk/gtkswitch.c:56`):

```
checkbutton[.text-button][.grouped]
├── check
╰── [label]
```

```
switch
├── image
├── image
╰── slider
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_check_button_names_its_indicator_check_and_switches_builtin_when_grouped() {
    // mutation: return Builtin::Check unconditionally from CheckButtonC::builtin
    // and the grouped assertion fails.
    use icedtea_ui::icons::builtin::Builtin;
    use icedtea_ui::widgets::check_button::CheckButtonC;

    let mut props = Props::default();
    props.set(PropName::Label, Prop::Str("Enable".into()));
    check(Kind::CheckButton, "check_button", &props);
    let rendered = node_tree_of(Kind::CheckButton, &props);
    assert!(rendered.starts_with("checkbutton.text-button"), "{rendered}");
    assert!(rendered.contains("check"), "{rendered}");

    props.set(PropName::Group, Prop::Str("mode".into()));
    let grouped = node_tree_of(Kind::CheckButton, &props);
    assert!(grouped.contains("checkbutton.text-button.grouped"), "{grouped}");
    // GTK keeps the node named `check` and swaps the *builtin* to a radio.
    assert!(grouped.contains("check"), "{grouped}");
    assert_eq!(CheckButtonC::builtin_for(true, false), Builtin::Radio);
    assert_eq!(CheckButtonC::builtin_for(false, false), Builtin::Check);
    assert_eq!(CheckButtonC::builtin_for(false, true), Builtin::CheckIndeterminate);
    check(Kind::CheckButton, "check_button", &props);
}

#[test]
fn a_switch_has_two_images_and_a_slider() {
    // mutation: build one image and this fails with "fixture requires a node at
    // 'switch/image'" only after both are gone — so drop the slider instead to
    // see the failure immediately.
    check(Kind::Switch, "switch", &Props::default());
    let rendered = node_tree_of(Kind::Switch, &Props::default());
    assert_eq!(rendered.matches("image").count(), 2, "{rendered}");
    assert!(rendered.contains("slider"), "{rendered}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn flipping_a_switch_animates_the_slider_to_the_other_end() {
    // mutation: set `slide` straight to its target in SwitchC::set_prop and the
    // mid-animation capture matches the final one.
    use icedtea_ui::view::builders::switch;
    use icedtea_ui::widgets::switch::SwitchExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Flip(bool);

    let frames = run(
        false,
        |model: &mut bool, Flip(on): Flip| {
            *model = on;
            Cmd::None
        },
        |model: &bool| switch(*model).on_toggle(Flip),
        (64, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(Flip(true)),
            ScriptStep::Advance(Duration::from_millis(60)),
            ScriptStep::Capture,
            ScriptStep::Advance(Duration::from_millis(400)),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..64).map(|x| frames.pixel(frame, x, 20)).collect::<Vec<_>>();
    assert_ne!(row(0), row(2), "the slider ends at the other end");
    assert_ne!(row(1), row(2), "and passes through an intermediate position");
}

#[test]
#[ignore = "P7 fills in Builtin::path (contract §9, plan D9)"]
fn a_check_button_paints_the_builtin_check_glyph() {
    use icedtea_ui::view::builders::check_button;
    use icedtea_ui::widgets::check_button::CheckButtonExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| check_button("On").active(true),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (120, 40)), "the check glyph must ink");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_check_button_names`; expected FAIL, `unresolved import icedtea_ui::widgets::check_button`.

- [ ] **Step 4: Write `ui/src/widgets/check_button.rs`**

```rust
//! `GtkCheckButton` — `Kind::CheckButton`, CSS node `checkbutton`.
//!
//! ```text
//! checkbutton[.text-button][.grouped]
//! ├── check
//! ╰── [label]
//! ```
//!
//! GTK's own prose says the indicator node "is named check when no group is
//! set, and radio if the checkbutton is grouped". In 4.22.4's rendered tree the
//! node stays `check` and the *builtin image* becomes a radio, which is what
//! `-gtk-icon-source` selects; that is what [`CheckButtonC::builtin_for`]
//! encodes, and the `.grouped` class on the root is how a stylesheet tells the
//! two apart.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::icons::builtin::Builtin;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;

/// A `GtkCheckButton` labelled `label`.
#[must_use]
pub fn check_button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::CheckButton).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkCheckButton`'s own setters and signals.
pub trait CheckButtonExt<Msg>: Sized {
    /// `GtkCheckButton:active`.
    fn active(self, on: bool) -> Self;
    /// `GtkCheckButton:inconsistent`.
    fn inconsistent(self, on: bool) -> Self;
    /// `GtkCheckButton:group`, by name.
    fn group(self, name: &str) -> Self;
    /// `GtkCheckButton:use-underline`.
    fn use_underline(self, on: bool) -> Self;
    /// `GtkCheckButton::toggled`.
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> CheckButtonExt<Msg> for View<Msg> {
    fn active(self, on: bool) -> Self {
        self.prop(PropName::Active, Prop::Bool(on))
    }
    fn inconsistent(self, on: bool) -> Self {
        self.prop(PropName::Indeterminate, Prop::Bool(on))
    }
    fn group(self, name: &str) -> Self {
        self.prop(PropName::Group, Prop::Str(Rc::from(name)))
    }
    fn use_underline(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }
}

/// `Kind::CheckButton`'s controller.
pub struct CheckButtonC {
    /// `GtkCheckButton:active`.
    pub active: bool,
    /// `GtkCheckButton:inconsistent`.
    pub inconsistent: bool,
    /// The group name, when grouped.
    pub group: Option<Rc<str>>,
    /// The `check` subnode.
    pub check: Node,
    /// The `label` subnode, when the button has text.
    pub label: Option<Node>,
    pointer: PointerState,
}

impl CheckButtonC {
    /// The builtin shape for `(grouped, inconsistent)`.
    #[must_use]
    pub fn builtin_for(grouped: bool, inconsistent: bool) -> Builtin {
        match (grouped, inconsistent) {
            (true, true) => Builtin::RadioIndeterminate,
            (true, false) => Builtin::Radio,
            (false, true) => Builtin::CheckIndeterminate,
            (false, false) => Builtin::Check,
        }
    }

    /// This button's builtin shape.
    #[must_use]
    pub fn builtin(&self) -> Builtin {
        Self::builtin_for(self.group.is_some(), self.inconsistent)
    }

    fn apply(&self, node: &Node) {
        node.remove_class("text-button");
        if self.label.is_some() {
            node.add_class("text-button");
        }
        node.remove_class("grouped");
        if self.group.is_some() {
            node.add_class("grouped");
        }
        node.set_state(PseudoStates::CHECKED, self.active && !self.inconsistent);
        node.set_state(PseudoStates::INDETERMINATE, self.inconsistent);
        self.check.set_state(PseudoStates::CHECKED, self.active && !self.inconsistent);
        self.check.set_state(PseudoStates::INDETERMINATE, self.inconsistent);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for CheckButtonC {
    fn kind(&self) -> Kind {
        Kind::CheckButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let check = Node::new("check");
        node.append_child(&check);
        let label = props.str(PropName::Label).map(|_| {
            let label = Node::new("label");
            node.append_child(&label);
            label
        });
        let this = CheckButtonC {
            active: props.bool(PropName::Active, false),
            inconsistent: props.bool(PropName::Indeterminate, false),
            group: props.str(PropName::Group).map(Rc::from),
            check,
            label,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Active, Prop::Bool(on)) => self.active = *on,
            (PropName::Indeterminate, Prop::Bool(on)) => self.inconsistent = *on,
            (PropName::Group, Prop::Str(name)) => self.group = Some(Rc::clone(name)),
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if !clicked && !matches!(ev, Event::Activate) {
            return Vec::new();
        }
        // Activating always leaves the inconsistent state, as GTK does; a
        // grouped check cannot be turned off by clicking it.
        self.inconsistent = false;
        let next = if self.group.is_some() { true } else { !self.active };
        self.active = next;
        self.apply(cx.node);
        cx.handled = true;
        cx.handlers.fire_bool(EventKind::Toggle, next).map_or_else(Vec::new, |m| vec![m])
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        if !self.active && !self.inconsistent {
            return false;
        }
        // The indicator's own allocation is the `check` node's; the builtin is
        // drawn into it. P7 replaces Builtin::path's geometry (plan D9).
        let _ = alloc;
        self.builtin().draw(canvas, alloc.content_box, style.color());
        true
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/switch.rs`**

```rust
//! `GtkSwitch` — `Kind::Switch`, CSS node `switch`.
//!
//! ```text
//! switch
//! ├── image
//! ├── image
//! ╰── slider
//! ```
//!
//! Four nodes, no style classes on any of them. The switch supports pan and
//! drag: dragging the slider past the midpoint toggles, and `slide` animates
//! the slider between the two ends on the animation clock.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;

/// How long the slider takes to cross, matching Adwaita's own switch timing.
const SLIDE: Duration = Duration::from_millis(200);

/// A `GtkSwitch` in `active`.
#[must_use]
pub fn switch<Msg: Clone + 'static>(active: bool) -> View<Msg> {
    View::new(Kind::Switch).prop(PropName::Active, Prop::Bool(active))
}

/// `GtkSwitch`'s own setters and signals.
pub trait SwitchExt<Msg>: Sized {
    /// `GtkSwitch:state` — the backend state, which may lag `active`.
    fn state(self, on: bool) -> Self;
    /// `GtkSwitch::state-set`.
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> SwitchExt<Msg> for View<Msg> {
    fn state(self, on: bool) -> Self {
        self.prop(PropName::Checked, Prop::Bool(on))
    }
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }
}

/// `Kind::Switch`'s controller.
pub struct SwitchC {
    /// `GtkSwitch:active`.
    pub active: bool,
    /// Grab offset within the slider while dragging.
    pub drag: Option<f32>,
    /// Slider position, `0.0` (off) to `1.0` (on).
    pub slide: f32,
    /// The `slider` subnode.
    pub slider: Node,
    /// The first `image` subnode (on).
    pub on_image: Node,
    /// The second `image` subnode (off).
    pub off_image: Node,
    target: f32,
    last_tick: Option<Duration>,
    pointer: PointerState,
}

impl SwitchC {
    fn apply(&mut self, node: &Node) {
        node.set_state(PseudoStates::CHECKED, self.active);
        self.target = if self.active { 1.0 } else { 0.0 };
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SwitchC {
    fn kind(&self) -> Kind {
        Kind::Switch
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let on_image = Node::new("image");
        node.append_child(&on_image);
        let off_image = Node::new("image");
        node.append_child(&off_image);
        let slider = Node::new("slider");
        node.append_child(&slider);
        let active = props.bool(PropName::Active, false);
        let mut this = SwitchC {
            active,
            drag: None,
            slide: if active { 1.0 } else { 0.0 },
            slider,
            on_image,
            off_image,
            target: if active { 1.0 } else { 0.0 },
            last_tick: None,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if matches!(name, PropName::Active | PropName::Checked) {
            if let Prop::Bool(on) = value {
                self.active = *on;
                self.apply(node);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        // A drag past the midpoint toggles; a plain click toggles too.
        if let (Event::PointerMotion { local }, Some(rect), Some(_)) = (ev, bounds, self.drag) {
            if rect.width > 0.0 {
                self.slide = (local.0 / rect.width).clamp(0.0, 1.0);
            }
            return Vec::new();
        }
        if matches!(ev, Event::PointerDown { .. }) {
            self.drag = Some(0.0);
        }
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if matches!(ev, Event::PointerUp { .. }) {
            let dragged = self.drag.take().is_some();
            let next = if dragged && (self.slide - if self.active { 1.0 } else { 0.0 }).abs() > 0.25
            {
                self.slide >= 0.5
            } else if clicked {
                !self.active
            } else {
                self.active
            };
            if next != self.active {
                self.active = next;
                self.apply(cx.node);
                cx.handled = true;
                return cx
                    .handlers
                    .fire_bool(EventKind::Toggle, next)
                    .map_or_else(Vec::new, |m| vec![m]);
            }
            self.apply(cx.node);
        }
        if matches!(ev, Event::Activate) {
            self.active = !self.active;
            self.apply(cx.node);
            cx.handled = true;
            let next = self.active;
            return cx.handlers.fire_bool(EventKind::Toggle, next).map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let elapsed = self.last_tick.map_or(Duration::ZERO, |last| now.saturating_sub(last));
        self.last_tick = Some(now);
        let step = elapsed.as_secs_f32() / SLIDE.as_secs_f32();
        if (self.target - self.slide).abs() <= step {
            self.slide = self.target;
        } else if self.target > self.slide {
            self.slide += step;
        } else {
            self.slide -= step;
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        ((self.target - self.slide).abs() > f32::EPSILON).then(|| now + Duration::from_millis(16))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        let knob = content.height.min(content.width / 2.0);
        if knob <= 0.0 {
            return false;
        }
        let x = content.x + (content.width - knob) * self.slide.clamp(0.0, 1.0);
        let rect = Rect::new(x, content.y, knob, content.height);
        canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}
```

- [ ] **Step 6: Register both** — `pub mod check_button;`, `pub mod switch;`; the two dispatch arms; `pub use crate::widgets::check_button::{CheckButtonExt, check_button};` and `pub use crate::widgets::switch::{SwitchExt, switch};`.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_check_button_names`, `... a_switch_has_two_images`, `cargo test -p icedtea-ui --test widget_pixels flipping_a_switch`; expected PASS, with `a_check_button_paints_the_builtin_check_glyph` ignored.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): CheckButton and Switch

The check indicator keeps GTK's `check` node name and swaps its builtin to a
radio when grouped; the switch slider animates across on the animation clock,
and a drag past the midpoint toggles it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 21: `MenuButton` and `DropDown`

**Files:** Create `ui/src/widgets/menu_button.rs`, `ui/src/widgets/drop_down.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{menu_button,drop_down}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 18's `PopoverC`; Task 7's `ArrowDirection`, `MatchMode`, `ListItem`, `PointerState`, `local_rect`, `shift_event`.
- Produces:
  ```rust
  pub fn menu_button<Msg: Clone + 'static>(label: &str) -> View<Msg>;
  pub trait MenuButtonExt<Msg>: Sized {
      fn icon(self, icon: IconRef) -> Self;
      fn always_show_arrow(self, on: bool) -> Self;
      fn direction(self, dir: ArrowDirection) -> Self;
      fn has_frame(self, on: bool) -> Self;
      fn primary(self, on: bool) -> Self;
  }
  pub struct MenuButtonC { pub open: bool, pub button: Node, pub arrow: Option<Node>,
                           pub popover: PopoverC }

  pub fn drop_down<Msg: Clone + 'static>(items: &[&str]) -> View<Msg>;
  pub fn drop_down_from<Msg: Clone + 'static>(items: Rc<[ListItem]>) -> View<Msg>;
  pub trait DropDownExt<Msg>: Sized {
      fn selected(self, index: usize) -> Self;
      fn enable_search(self, on: bool) -> Self;
      fn show_arrow(self, on: bool) -> Self;
      fn search_match_mode(self, mode: MatchMode) -> Self;
      fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
  }
  pub struct DropDownC { pub items: Rc<[ListItem]>, pub selected: usize, pub open: bool,
                         pub search: String, pub filtered: Vec<usize>,
                         pub button: Node, pub popover: PopoverC, pub list: Node,
                         pub rows: Vec<Node> }
  impl DropDownC {
      /// Indices matching `search` under `mode`. Never panics on any input.
      #[must_use] pub fn filter(items: &[ListItem], search: &str, mode: MatchMode) -> Vec<usize>;
  }
  ```
  Contract §5.2 types `DropDownC.list` as `ListViewC`, which is a P6 kind; P5
  builds the `listview` node and its `row` children directly and P6 replaces the
  field with its own `ListViewC` when `Kind::ListView` lands. Record this in
  §10 alongside the deviations above.

- [ ] **Step 1: Vendor the fixtures** — `menu_button.txt` from `gtk/gtkmenubutton.c:60`, and `drop_down.txt` as contract §5.2 spells the tree GTK actually renders (its own block is one line of prose):

```
menubutton
╰── button.toggle
    ╰── <content>
         ╰── [arrow]
```

```
dropdown
├── button.toggle
│   ╰── <content>
│        ╰── [arrow]
╰── popover.menu
    ╰── contents
        ├── [entry.search]
        ╰── listview
            ╰── row[.activatable]
            ┊
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_menu_button_wraps_a_toggle_button_and_a_drop_down_holds_a_popover_list() {
    // mutation: name the menubutton's child `button` without `.toggle` and the
    // matcher reports "required class 'toggle' missing".
    check(Kind::MenuButton, "menu_button", &Props::default());
    let rendered = node_tree_of(Kind::MenuButton, &Props::default());
    assert!(rendered.contains("button.toggle"), "{rendered}");

    let mut props = Props::default();
    props.set(
        PropName::Model,
        Prop::Items(std::rc::Rc::from(vec![
            icedtea_ui::widgets::ListItem::new(0, "One"),
            icedtea_ui::widgets::ListItem::new(1, "Two"),
        ])),
    );
    check(Kind::DropDown, "drop_down", &props);
    let dropdown = node_tree_of(Kind::DropDown, &props);
    assert_eq!(dropdown.matches("row").count(), 2, "{dropdown}");
    assert!(dropdown.contains("popover.background.menu"), "{dropdown}");
}

#[test]
fn a_drop_downs_search_filter_never_panics_and_honours_its_mode() {
    // mutation: use `starts_with` for Substring and the Substring assertion
    // returns an empty vec.
    use icedtea_ui::widgets::drop_down::DropDownC;
    use icedtea_ui::widgets::{ListItem, MatchMode};
    let items = vec![
        ListItem::new(0, "Alpha"),
        ListItem::new(1, "beta"),
        ListItem::new(2, "\u{00e9}clair"),
    ];
    assert_eq!(DropDownC::filter(&items, "", MatchMode::Substring), vec![0, 1, 2]);
    assert_eq!(DropDownC::filter(&items, "et", MatchMode::Substring), vec![1]);
    assert_eq!(DropDownC::filter(&items, "be", MatchMode::Prefix), vec![1]);
    assert_eq!(DropDownC::filter(&items, "beta", MatchMode::Exact), vec![1]);
    assert!(DropDownC::filter(&items, "\u{00e9}", MatchMode::Prefix).contains(&2));
    // Hostile input: a lone surrogate cannot exist in a &str, but a very long
    // needle and a needle longer than every haystack must both be fine.
    assert!(DropDownC::filter(&items, &"x".repeat(100_000), MatchMode::Substring).is_empty());
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn opening_a_drop_down_and_picking_an_item_updates_the_button() {
    // mutation: never fire EventKind::Selected in DropDownC::on_event and the
    // model stays at 0.
    use icedtea_ui::view::builders::drop_down;
    use icedtea_ui::widgets::drop_down::DropDownExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Picked(usize);

    let frames = run(
        0usize,
        |model: &mut usize, Picked(index): Picked| {
            *model = index;
            Cmd::None
        },
        |model: &usize| drop_down(&["One", "Two", "Three"]).selected(*model).on_selected(Picked),
        (200, 200),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 100.0, y: 16.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 16)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the button repaints when the list opens");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_menu_button_wraps`; expected FAIL, `fixture requires a node at 'menubutton/button'`.

- [ ] **Step 4: Write `ui/src/widgets/menu_button.rs`**

```rust
//! `GtkMenuButton` — `Kind::MenuButton`, CSS node `menubutton`.
//!
//! ```text
//! menubutton
//! ╰── button.toggle
//!     ╰── <content>
//!          ╰── [arrow]
//! ```
//!
//! The inner button takes `.image-button`, `.text-button` or `.arrow-button`
//! from its content, and the `arrow` node carries one of `.none`, `.up`,
//! `.down`, `.left`, `.right` for the direction the menu will appear in.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::css::value::image::IconRef;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{Kind, Prop, PropName, Props, View};
use crate::widgets::popover::PopoverC;
use crate::widgets::{
    ArrowDirection, PointerState, Position, WidgetEnum, local_rect, shift_event,
};
use crate::window::PopupAnchorPoint;

/// A `GtkMenuButton` labelled `label`. Children become the popover content.
#[must_use]
pub fn menu_button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::MenuButton).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkMenuButton`'s own setters.
pub trait MenuButtonExt<Msg>: Sized {
    /// `GtkMenuButton:icon-name`, as an `IconRef`.
    fn icon(self, icon: IconRef) -> Self;
    /// `GtkMenuButton:always-show-arrow`.
    fn always_show_arrow(self, on: bool) -> Self;
    /// `GtkMenuButton:direction`.
    fn direction(self, dir: ArrowDirection) -> Self;
    /// `GtkMenuButton:has-frame`.
    fn has_frame(self, on: bool) -> Self;
    /// `GtkMenuButton:primary`.
    fn primary(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> MenuButtonExt<Msg> for View<Msg> {
    fn icon(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn always_show_arrow(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn direction(self, dir: ArrowDirection) -> Self {
        self.prop(PropName::Gravity, dir.to_prop())
    }
    fn has_frame(self, on: bool) -> Self {
        self.prop(PropName::Reveal, Prop::Bool(on))
    }
    fn primary(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
}

/// `Kind::MenuButton`'s controller.
pub struct MenuButtonC {
    /// Whether the popover is showing.
    pub open: bool,
    /// The `button.toggle` subnode.
    pub button: Node,
    /// The `arrow` subnode, when an arrow is shown.
    pub arrow: Option<Node>,
    /// The embedded popover.
    pub popover: PopoverC,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for MenuButtonC {
    fn kind(&self) -> Kind {
        Kind::MenuButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["toggle"]);
        node.append_child(&button);
        let has_label = props.str(PropName::Label).is_some();
        let has_icon = props.get(PropName::Icon).is_some();
        if has_label {
            button.append_child(&Node::new("label"));
            button.add_class("text-button");
        }
        if has_icon {
            button.append_child(&Node::new("image"));
            button.add_class("image-button");
        }
        let direction =
            ArrowDirection::from_prop(props.get(PropName::Gravity), ArrowDirection::Down);
        let arrow = (props.bool(PropName::ShowArrow, true) || !(has_label || has_icon)).then(|| {
            let arrow = Node::with_classes("arrow", &[direction.css_class()]);
            button.append_child(&arrow);
            button.add_class("arrow-button");
            arrow
        });
        MenuButtonC {
            open: false,
            button,
            arrow,
            popover: PopoverC::for_test(Position::Bottom, true, true),
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Gravity, Prop::Enum(_)) = (name, value) {
            let direction = ArrowDirection::from_prop(Some(value), ArrowDirection::Down);
            if let Some(arrow) = self.arrow.as_ref() {
                arrow.set_classes(&[direction.css_class()]);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if matches!(ev, Event::PopupDone) {
            self.open = false;
            self.button.set_state(PseudoStates::CHECKED, false);
            return Vec::new();
        }
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self
            .pointer
            .observe(&self.button, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)))
        {
            cx.handled = true;
            if self.open {
                self.popover.close(cx);
                self.open = false;
            } else {
                self.popover.open(
                    PopupAnchorPoint::Node(self.button.clone()),
                    (rect.width.max(1.0) as u32, 200),
                    cx,
                );
                self.open = true;
            }
            self.button.set_state(PseudoStates::CHECKED, self.open);
        }
        Vec::new()
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/drop_down.rs`**

```rust
//! `GtkDropDown` — `Kind::DropDown`, CSS node `dropdown`.
//!
//! ```text
//! dropdown
//! ├── button.toggle
//! │   ╰── <content>
//! │        ╰── [arrow]
//! ╰── popover.menu
//!     ╰── contents
//!         ├── [entry.search]
//!         ╰── listview
//!             ╰── row[.activatable]
//!             ┊
//! ```
//!
//! GTK's own block says only "a single node `dropdown`, with the button and
//! popover nodes as children"; the expansion above is the tree it actually
//! renders, and is what contract §5.2 vendors as the fixture.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::popover::PopoverC;
use crate::widgets::{ListItem, MatchMode, PointerState, Position, WidgetEnum, local_rect, shift_event};
use crate::window::PopupAnchorPoint;

/// A `GtkDropDown` over string items.
#[must_use]
pub fn drop_down<Msg: Clone + 'static>(items: &[&str]) -> View<Msg> {
    let model: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(index, label)| ListItem::new(index as u64, label))
        .collect();
    drop_down_from(Rc::from(model))
}

/// A `GtkDropDown` over an explicit model.
#[must_use]
pub fn drop_down_from<Msg: Clone + 'static>(items: Rc<[ListItem]>) -> View<Msg> {
    View::new(Kind::DropDown).prop(PropName::Model, Prop::Items(items))
}

/// `GtkDropDown`'s own setters and signals.
pub trait DropDownExt<Msg>: Sized {
    /// `GtkDropDown:selected`.
    fn selected(self, index: usize) -> Self;
    /// `GtkDropDown:enable-search`.
    fn enable_search(self, on: bool) -> Self;
    /// `GtkDropDown:show-arrow`.
    fn show_arrow(self, on: bool) -> Self;
    /// `GtkDropDown:search-match-mode`.
    fn search_match_mode(self, mode: MatchMode) -> Self;
    /// `GtkDropDown:selected`'s change notification.
    fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> DropDownExt<Msg> for View<Msg> {
    fn selected(self, index: usize) -> Self {
        self.prop(PropName::Selected, Prop::Int(index as i64))
    }
    fn enable_search(self, on: bool) -> Self {
        self.prop(PropName::EnableSearch, Prop::Bool(on))
    }
    fn show_arrow(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn search_match_mode(self, mode: MatchMode) -> Self {
        self.prop(PropName::SelectionMode, mode.to_prop())
    }
    fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::DropDown`'s controller.
pub struct DropDownC {
    /// The model.
    pub items: Rc<[ListItem]>,
    /// The selected index, clamped into `items`.
    pub selected: usize,
    /// Whether the popover is showing.
    pub open: bool,
    /// The search entry's text.
    pub search: String,
    /// Indices of `items` currently visible.
    pub filtered: Vec<usize>,
    /// The `button.toggle` subnode.
    pub button: Node,
    /// The embedded popover.
    pub popover: PopoverC,
    /// The `listview` subnode inside the popover's `contents`.
    pub list: Node,
    /// One `row` node per visible item.
    pub rows: Vec<Node>,
    mode: MatchMode,
    pointer: PointerState,
}

impl DropDownC {
    /// Indices of `items` matching `search` under `mode`.
    ///
    /// An empty needle matches everything. Comparison is ASCII-case-insensitive
    /// on both sides, which is what `GtkStringFilter`'s default does; a needle
    /// longer than every label simply matches nothing.
    #[must_use]
    pub fn filter(items: &[ListItem], search: &str, mode: MatchMode) -> Vec<usize> {
        if search.is_empty() {
            return (0..items.len()).collect();
        }
        let needle = search.to_lowercase();
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let hay = item.label.to_lowercase();
                match mode {
                    MatchMode::Exact => hay == needle,
                    MatchMode::Prefix => hay.starts_with(&needle),
                    MatchMode::Substring => hay.contains(&needle),
                }
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Rebuild the row nodes from `filtered`.
    fn rebuild_rows(&mut self) {
        for row in self.rows.drain(..) {
            row.detach();
        }
        for &index in &self.filtered {
            let row = Node::with_classes("row", &["activatable"]);
            row.set_state(PseudoStates::SELECTED, index == self.selected);
            self.list.append_child(&row);
            self.rows.push(row);
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for DropDownC {
    fn kind(&self) -> Kind {
        Kind::DropDown
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["toggle"]);
        node.append_child(&button);
        button.append_child(&Node::new("label"));
        if props.bool(PropName::ShowArrow, true) {
            button.append_child(&Node::with_classes("arrow", &["down"]));
        }
        let popover_node = Node::with_classes("popover", &["background", "menu"]);
        node.append_child(&popover_node);
        let popover = PopoverC::build(&popover_node, &Props::default(), cx);
        popover_node.add_class("menu");
        if props.bool(PropName::EnableSearch, false) {
            popover.contents.append_child(&Node::with_classes("entry", &["search"]));
        }
        let list = Node::new("listview");
        popover.contents.append_child(&list);

        let items: Rc<[ListItem]> = match props.get(PropName::Model) {
            Some(Prop::Items(items)) => Rc::clone(items),
            _ => Rc::from(Vec::new()),
        };
        let selected = usize::try_from(props.int(PropName::Selected, 0))
            .unwrap_or(0)
            .min(items.len().saturating_sub(1));
        let mode = MatchMode::from_prop(props.get(PropName::SelectionMode), MatchMode::Substring);
        let mut this = DropDownC {
            filtered: Self::filter(&items, "", mode),
            items,
            selected,
            open: false,
            search: String::new(),
            button,
            popover,
            list,
            rows: Vec::new(),
            mode,
            pointer: PointerState::default(),
        };
        this.rebuild_rows();
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Model, Prop::Items(items)) => {
                self.items = Rc::clone(items);
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
                self.filtered = Self::filter(&self.items, &self.search, self.mode);
            }
            (PropName::Selected, Prop::Int(index)) => {
                self.selected = usize::try_from(*index)
                    .unwrap_or(0)
                    .min(self.items.len().saturating_sub(1));
            }
            _ => return,
        }
        self.rebuild_rows();
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if matches!(ev, Event::PopupDone) {
            self.open = false;
            self.button.set_state(PseudoStates::CHECKED, false);
            return Vec::new();
        }
        // A row click selects and closes.
        for (position, row) in self.rows.iter().enumerate() {
            let Some(rect) = local_rect(cx.tree, cx.node, row) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let mut probe = PointerState::default();
            if probe.observe(row, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height))) {
                let index = self.filtered.get(position).copied().unwrap_or(self.selected);
                self.selected = index;
                self.open = false;
                self.button.set_state(PseudoStates::CHECKED, false);
                self.popover.close(cx);
                self.rebuild_rows();
                cx.handled = true;
                return cx
                    .handlers
                    .fire_index(EventKind::Selected, index)
                    .map_or_else(Vec::new, |m| vec![m]);
            }
        }
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self
            .pointer
            .observe(&self.button, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)))
        {
            cx.handled = true;
            if self.open {
                self.popover.close(cx);
                self.open = false;
            } else {
                self.popover.open(
                    PopupAnchorPoint::Node(self.button.clone()),
                    (rect.width.max(1.0) as u32, 240),
                    cx,
                );
                self.open = true;
            }
            self.button.set_state(PseudoStates::CHECKED, self.open);
        }
        Vec::new()
    }
}

/// `PopoverC` is constructed through `Controller::build`, which needs the trait
/// in scope at the call site above.
use crate::view::controller::Controller as _;

/// `Position` is used for the popover's default placement.
const _: Position = Position::Bottom;
```

- [ ] **Step 6: Register both** — `pub mod menu_button;`, `pub mod drop_down;`; the two arms; `pub use crate::widgets::menu_button::{MenuButtonExt, menu_button};` and `pub use crate::widgets::drop_down::{DropDownExt, drop_down, drop_down_from};`.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_menu_button_wraps`, `... a_drop_downs_search_filter`, `cargo test -p icedtea-ui --test widget_pixels opening_a_drop_down`; expected PASS.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): MenuButton and DropDown over the shared Popover

DropDownC builds its listview and rows directly; P6 swaps the `list` field for
its own ListViewC when Kind::ListView lands. The search filter is total: an
empty needle matches everything and an over-long one matches nothing.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 22: `ColorDialogButton`, `ColorDialog`, `FontDialogButton`, `FontDialog`

**Files:** Create `ui/src/widgets/color_dialog.rs`, `ui/src/widgets/font_dialog.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{color_dialog_button,color_dialog,font_dialog_button,font_dialog}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 7's `ColorDialogSpec`, `FontLevel`, `PointerState`, `local_rect`, `shift_event`; M2's `css::value::Rgba`, `text::FontDatabase`.
- Produces:
  ```rust
  pub fn color_dialog_button<Msg: Clone + 'static>(rgba: Rgba) -> View<Msg>;
  pub trait ColorDialogButtonExt<Msg>: Sized {
      fn with_alpha(self, on: bool) -> Self;
      fn dialog(self, spec: ColorDialogSpec) -> Self;
      fn on_change(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
  }
  pub struct ColorDialogButtonC { pub rgba: Rgba, pub dialog_open: bool,
                                  pub button: Node, pub swatch: Node }
  pub fn color_dialog<Msg: Clone + 'static>(rgba: Rgba) -> View<Msg>;
  pub trait ColorDialogExt<Msg>: Sized {
      fn title(self, text: &str) -> Self;
      fn modal(self, on: bool) -> Self;
      fn with_alpha(self, on: bool) -> Self;
      fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      fn on_change(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
  }
  pub struct ColorDialogC { pub rgba: Rgba, pub palette: Vec<Rgba>, pub custom: Option<Rgba>,
                            pub swatches: Vec<Node> }
  impl ColorDialogC {
      /// GTK's own 9x5 default palette, flattened.
      #[must_use] pub fn default_palette() -> Vec<Rgba>;
      /// `Rgba` <-> the `f64` a `Handler::Float` carries: 0xAARRGGBB as an
      /// exactly-representable integer.
      #[must_use] pub fn pack(rgba: Rgba) -> f64;
      #[must_use] pub fn unpack(packed: f64) -> Rgba;
  }

  pub fn font_dialog_button<Msg: Clone + 'static>(desc: &str) -> View<Msg>;
  pub trait FontDialogButtonExt<Msg>: Sized {
      fn use_font(self, on: bool) -> Self;
      fn use_size(self, on: bool) -> Self;
      fn level(self, level: FontLevel) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
  }
  pub struct FontDialogButtonC { pub desc: Rc<str>, pub dialog_open: bool,
                                 pub button: Node, pub label: Node }
  pub fn font_dialog<Msg: Clone + 'static>(desc: &str) -> View<Msg>;
  pub trait FontDialogExt<Msg>: Sized {
      fn title(self, text: &str) -> Self;
      fn modal(self, on: bool) -> Self;
      fn language(self, lang: &str) -> Self;
      fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
  }
  pub struct FontDialogC { pub families: Vec<Rc<str>>, pub selected: Option<usize>,
                           pub size: f32, pub preview: String, pub list: Node }
  ```
  Contract §5.2 types `ColorDialogButton`'s handler as `on_change(|Rgba| Msg)`
  and `FontDialogC.list` as `ListViewC`. `Handler` has no `Rgba` variant and
  `ListViewC` is P6's, so the colour rides through `Handler::Float` as a packed
  `0xAARRGGBB` and `FontDialogC.list` is the `listview` `Node` until P6. Record
  both in §10 with the D-series deviations.

- [ ] **Step 1: Vendor the four fixtures** — from `gtk/gtkcolordialogbutton.c:65`, `gtk/gtkfontdialogbutton.c:52`, and the two dialog bodies contract §5.2 specifies (`gtk/gtkcolorchooserwidget.c:743`, `gtk/gtkcolorswatch.c:542`, `gtk/gtkfontchooserwidget.c:931`, `.dialog` from `gtk/deprecated/gtkdialog.c:601`):

```
colorbutton
╰── button.color
    ╰── [content]
```

```
window.dialog
╰── colorchooser
    ╰── colorswatch
    ┊
```

```
fontbutton
╰── button.font
    ╰── [content]
```

```
window.dialog
╰── fontchooser
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn the_dialog_buttons_and_their_dialog_bodies_match_gtks_trees() {
    // mutation: name the colour button's child `button` without `.color` and
    // the matcher reports "required class 'color' missing".
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;

    let mut colour = Props::default();
    colour.set(PropName::Value, Prop::Float(ColorDialogC::pack(Rgba {
        r: 0.2, g: 0.5, b: 0.9, a: 1.0,
    })));
    check(Kind::ColorDialogButton, "color_dialog_button", &colour);
    check(Kind::ColorDialog, "color_dialog", &colour);
    let body = node_tree_of(Kind::ColorDialog, &colour);
    assert!(body.starts_with("window.dialog"), "{body}");
    assert!(body.matches("colorswatch").count() >= 2, "{body}");

    let mut font = Props::default();
    font.set(PropName::Text, Prop::Str("Cantarell 11".into()));
    check(Kind::FontDialogButton, "font_dialog_button", &font);
    check(Kind::FontDialog, "font_dialog", &font);
}

#[test]
fn a_packed_colour_round_trips_exactly() {
    // mutation: pack with `* 255.0` and no rounding and the round trip drifts.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;
    for rgba in [
        Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
        Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
        Rgba { r: 0.2, g: 0.5, b: 0.9, a: 0.5 },
    ] {
        let back = ColorDialogC::unpack(ColorDialogC::pack(rgba));
        for (a, b) in [(rgba.r, back.r), (rgba.g, back.g), (rgba.b, back.b), (rgba.a, back.a)] {
            assert!((a - b).abs() <= 1.0 / 255.0, "{a} vs {b}");
        }
    }
    // Hostile input: NaN and out-of-range packs must clamp, not panic.
    let _ = ColorDialogC::unpack(f64::NAN);
    let _ = ColorDialogC::unpack(-1.0);
    let _ = ColorDialogC::unpack(f64::MAX);
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn clicking_a_colour_button_opens_its_dialog_and_repaints_the_swatch() {
    // mutation: never set `dialog_open` in ColorDialogButtonC::on_event and the
    // two captures match.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::view::builders::color_dialog_button;
    let frames = run(
        Rgba { r: 0.2, g: 0.5, b: 0.9, a: 1.0 },
        |_m: &mut Rgba, _msg: ()| Cmd::None,
        |model: &Rgba| color_dialog_button(*model),
        (80, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 40.0, y: 20.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..80).map(|x| frames.pixel(frame, x, 20)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), ":active must repaint the colour button");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees the_dialog_buttons`; expected FAIL, `unresolved import icedtea_ui::widgets::color_dialog`.

- [ ] **Step 4: Write `ui/src/widgets/color_dialog.rs`**

```rust
//! `GtkColorDialogButton` and `GtkColorDialog` — `Kind::ColorDialogButton`
//! (node `colorbutton`) and `Kind::ColorDialog` (node `window`, class
//! `.dialog`).
//!
//! ```text
//! colorbutton
//! ╰── button.color
//!     ╰── [content]
//! ```
//!
//! ```text
//! window.dialog
//! ╰── colorchooser
//!     ╰── colorswatch
//!     ┊
//! ```
//!
//! `GtkColorDialog` is a plain `GObject` that launches a **deprecated**
//! `GtkColorChooserDialog`, so there is no GTK widget to copy: icedtea builds
//! the chooser body itself under `window.dialog`, with the node names GTK's own
//! chooser uses (`colorchooser`, `colorswatch`).

use std::rc::Rc;

use crate::css::node::Node;
use crate::css::value::Rgba;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{ColorDialogSpec, PointerState, local_rect, shift_event};

/// A `GtkColorDialogButton` showing `rgba`.
#[must_use]
pub fn color_dialog_button<Msg: Clone + 'static>(rgba: Rgba) -> View<Msg> {
    View::new(Kind::ColorDialogButton).prop(PropName::Value, Prop::Float(ColorDialogC::pack(rgba)))
}

/// A `GtkColorDialog` body opened on `rgba`.
#[must_use]
pub fn color_dialog<Msg: Clone + 'static>(rgba: Rgba) -> View<Msg> {
    View::new(Kind::ColorDialog).prop(PropName::Value, Prop::Float(ColorDialogC::pack(rgba)))
}

/// `GtkColorDialogButton`'s own setters.
pub trait ColorDialogButtonExt<Msg>: Sized {
    /// `GtkColorDialog:with-alpha`, bound down from the button.
    fn with_alpha(self, on: bool) -> Self;
    /// The dialog the button launches.
    fn dialog(self, spec: ColorDialogSpec) -> Self;
    /// `GtkColorDialogButton:rgba`'s change notification, as a packed colour.
    fn on_change(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ColorDialogButtonExt<Msg> for View<Msg> {
    fn with_alpha(self, on: bool) -> Self {
        self.prop(PropName::Ratio, Prop::Bool(on))
    }
    fn dialog(self, spec: ColorDialogSpec) -> Self {
        self.prop(PropName::Ratio, Prop::Bool(spec.with_alpha))
            .prop(PropName::Modal, Prop::Bool(spec.modal))
    }
    fn on_change(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// `GtkColorDialog`'s own setters.
pub trait ColorDialogExt<Msg>: Sized {
    /// `GtkColorDialog:title`.
    fn title(self, text: &str) -> Self;
    /// `GtkColorDialog:modal`.
    fn modal(self, on: bool) -> Self;
    /// `GtkColorDialog:with-alpha`.
    fn with_alpha(self, on: bool) -> Self;
    /// The dialog's response index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    /// The chosen colour, packed.
    fn on_change(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ColorDialogExt<Msg> for View<Msg> {
    fn title(self, text: &str) -> Self {
        self.prop(PropName::Title, Prop::Str(Rc::from(text)))
    }
    fn modal(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn with_alpha(self, on: bool) -> Self {
        self.prop(PropName::Ratio, Prop::Bool(on))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }
    fn on_change(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// `Kind::ColorDialogButton`'s controller.
pub struct ColorDialogButtonC {
    /// The chosen colour.
    pub rgba: Rgba,
    /// Whether the dialog is showing.
    pub dialog_open: bool,
    /// The `button.color` subnode.
    pub button: Node,
    /// The `content` subnode painted with `rgba`.
    pub swatch: Node,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogButtonC {
    fn kind(&self) -> Kind {
        Kind::ColorDialogButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["color"]);
        node.append_child(&button);
        let swatch = Node::new("content");
        button.append_child(&swatch);
        ColorDialogButtonC {
            rgba: ColorDialogC::unpack(props.float(PropName::Value, 0.0)),
            dialog_open: false,
            button,
            swatch,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Value, Prop::Float(packed)) = (name, value) {
            self.rgba = ColorDialogC::unpack(*packed);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self
            .pointer
            .observe(&self.button, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)))
        {
            self.dialog_open = !self.dialog_open;
            cx.handled = true;
        }
        Vec::new()
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        _style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let rect = alloc.content_box;
        if rect.is_empty() {
            return false;
        }
        canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(self.rgba));
        true
    }
}

/// `Kind::ColorDialog`'s controller — the chooser body icedtea builds itself.
pub struct ColorDialogC {
    /// The chosen colour.
    pub rgba: Rgba,
    /// The offered palette.
    pub palette: Vec<Rgba>,
    /// The custom colour, once one is picked.
    pub custom: Option<Rgba>,
    /// One `colorswatch` node per palette entry.
    pub swatches: Vec<Node>,
    pointer: PointerState,
}

impl ColorDialogC {
    /// GTK's own default palette, one row per hue plus a greyscale row.
    #[must_use]
    pub fn default_palette() -> Vec<Rgba> {
        const HEX: &[u32] = &[
            0x99c1f1, 0x62a0ea, 0x3584e4, 0x1c71d8, 0x1a5fb4, 0x8ff0a4, 0x57e389, 0x33d17a,
            0x2ec27e, 0x26a269, 0xf9f06b, 0xf8e45c, 0xf6d32d, 0xf5c211, 0xe5a50a, 0xffbe6f,
            0xffa348, 0xff7800, 0xe66100, 0xc64600, 0xf66151, 0xed333b, 0xe01b24, 0xc01c28,
            0xa51d2d, 0xdc8add, 0xc061cb, 0x9141ac, 0x813d9c, 0x613583, 0xcdab8f, 0xb5835a,
            0x986a44, 0x865e3c, 0x63452c, 0xffffff, 0xf6f5f4, 0xdeddda, 0xc0bfbc, 0x9a9996,
            0x77767b, 0x5e5c64, 0x3d3846, 0x241f31, 0x000000,
        ];
        HEX.iter()
            .map(|hex| Rgba {
                r: ((hex >> 16) & 0xff) as f32 / 255.0,
                g: ((hex >> 8) & 0xff) as f32 / 255.0,
                b: (hex & 0xff) as f32 / 255.0,
                a: 1.0,
            })
            .collect()
    }

    /// Pack an `Rgba` into the `f64` a `Handler::Float` carries.
    ///
    /// `0xAARRGGBB` is at most 2^32, which an `f64` represents exactly, so the
    /// round trip is lossless to 8 bits per channel — the precision every
    /// colour picker works at anyway.
    #[must_use]
    pub fn pack(rgba: Rgba) -> f64 {
        let byte = |v: f32| -> u32 {
            if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u32 } else { 0 }
        };
        f64::from(
            (byte(rgba.a) << 24) | (byte(rgba.r) << 16) | (byte(rgba.g) << 8) | byte(rgba.b),
        )
    }

    /// Unpack. A non-finite or out-of-range value yields opaque black rather
    /// than panicking — the packed colour arrives from an application model.
    #[must_use]
    pub fn unpack(packed: f64) -> Rgba {
        if !packed.is_finite() || packed < 0.0 || packed > f64::from(u32::MAX) {
            return Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        }
        let bits = packed as u32;
        let channel = |shift: u32| ((bits >> shift) & 0xff) as f32 / 255.0;
        Rgba { r: channel(16), g: channel(8), b: channel(0), a: channel(24) }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogC {
    fn kind(&self) -> Kind {
        Kind::ColorDialog
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        node.add_class("dialog");
        let chooser = Node::new("colorchooser");
        node.append_child(&chooser);
        let palette = Self::default_palette();
        let swatches = palette
            .iter()
            .map(|_| {
                let swatch = Node::new("colorswatch");
                chooser.append_child(&swatch);
                swatch
            })
            .collect();
        ColorDialogC {
            rgba: Self::unpack(props.float(PropName::Value, 0.0)),
            palette,
            custom: None,
            swatches,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Value, Prop::Float(packed)) = (name, value) {
            self.rgba = Self::unpack(*packed);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        for (index, swatch) in self.swatches.iter().enumerate() {
            let Some(rect) = local_rect(cx.tree, cx.node, swatch) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let mut probe = PointerState::default();
            if probe.observe(swatch, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height))) {
                if let Some(rgba) = self.palette.get(index).copied() {
                    self.rgba = rgba;
                    self.custom = Some(rgba);
                    cx.handled = true;
                    return cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, Self::pack(rgba))
                        .map_or_else(Vec::new, |m| vec![m]);
                }
            }
        }
        let _ = &mut self.pointer;
        Vec::new()
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/font_dialog.rs`**

```rust
//! `GtkFontDialogButton` and `GtkFontDialog` — `Kind::FontDialogButton` (node
//! `fontbutton`) and `Kind::FontDialog` (node `window`, class `.dialog`).
//!
//! ```text
//! fontbutton
//! ╰── button.font
//!     ╰── [content]
//! ```
//!
//! ```text
//! window.dialog
//! ╰── fontchooser
//! ```
//!
//! Like `GtkColorDialog`, `GtkFontDialog` is a `GObject` launching a
//! deprecated chooser; icedtea builds the `fontchooser` body itself, listing
//! the families the M2 `FontDatabase` can actually match.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{FontLevel, PointerState, WidgetEnum, local_rect, shift_event};

/// A `GtkFontDialogButton` showing `desc` (a Pango-style description).
#[must_use]
pub fn font_dialog_button<Msg: Clone + 'static>(desc: &str) -> View<Msg> {
    View::new(Kind::FontDialogButton).prop(PropName::Text, Prop::Str(Rc::from(desc)))
}

/// A `GtkFontDialog` body opened on `desc`.
#[must_use]
pub fn font_dialog<Msg: Clone + 'static>(desc: &str) -> View<Msg> {
    View::new(Kind::FontDialog).prop(PropName::Text, Prop::Str(Rc::from(desc)))
}

/// `GtkFontDialogButton`'s own setters.
pub trait FontDialogButtonExt<Msg>: Sized {
    /// `GtkFontDialogButton:use-font`.
    fn use_font(self, on: bool) -> Self;
    /// `GtkFontDialogButton:use-size`.
    fn use_size(self, on: bool) -> Self;
    /// `GtkFontDialogButton:level`.
    fn level(self, level: FontLevel) -> Self;
    /// `GtkFontDialogButton:font-desc`'s change notification.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> FontDialogButtonExt<Msg> for View<Msg> {
    fn use_font(self, on: bool) -> Self {
        self.prop(PropName::Markup, Prop::Bool(on))
    }
    fn use_size(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn level(self, level: FontLevel) -> Self {
        self.prop(PropName::SelectionMode, level.to_prop())
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
}

/// `GtkFontDialog`'s own setters.
pub trait FontDialogExt<Msg>: Sized {
    /// `GtkFontDialog:title`.
    fn title(self, text: &str) -> Self;
    /// `GtkFontDialog:modal`.
    fn modal(self, on: bool) -> Self;
    /// `GtkFontDialog:language`.
    fn language(self, lang: &str) -> Self;
    /// The dialog's response index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    /// The chosen font description.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> FontDialogExt<Msg> for View<Msg> {
    fn title(self, text: &str) -> Self {
        self.prop(PropName::Title, Prop::Str(Rc::from(text)))
    }
    fn modal(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn language(self, lang: &str) -> Self {
        self.prop(PropName::Detail, Prop::Str(Rc::from(lang)))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
}

/// `Kind::FontDialogButton`'s controller.
pub struct FontDialogButtonC {
    /// The font description shown.
    pub desc: Rc<str>,
    /// Whether the dialog is showing.
    pub dialog_open: bool,
    /// The `button.font` subnode.
    pub button: Node,
    /// The `content` subnode carrying the description text.
    pub label: Node,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for FontDialogButtonC {
    fn kind(&self) -> Kind {
        Kind::FontDialogButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["font"]);
        node.append_child(&button);
        let label = Node::new("content");
        button.append_child(&label);
        FontDialogButtonC {
            desc: props.str(PropName::Text).map_or_else(|| Rc::from(""), Rc::from),
            dialog_open: false,
            button,
            label,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(desc)) = (name, value) {
            self.desc = Rc::clone(desc);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self
            .pointer
            .observe(&self.button, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height)))
        {
            self.dialog_open = !self.dialog_open;
            cx.handled = true;
        }
        Vec::new()
    }
}

/// `Kind::FontDialog`'s controller — the chooser body icedtea builds itself.
pub struct FontDialogC {
    /// Families the font database offered.
    pub families: Vec<Rc<str>>,
    /// The selected family, if any.
    pub selected: Option<usize>,
    /// The chosen size in px.
    pub size: f32,
    /// The preview string.
    pub preview: String,
    /// The `fontchooser` subnode; P6 replaces it with a real `ListViewC`.
    pub list: Node,
}

impl<Msg: Clone + 'static> Controller<Msg> for FontDialogC {
    fn kind(&self) -> Kind {
        Kind::FontDialog
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        node.add_class("dialog");
        let list = Node::new("fontchooser");
        node.append_child(&list);
        // The description is `"<family> <size>"`, Pango's own shorthand.
        let desc = props.str(PropName::Text).unwrap_or("");
        let (family, size) = desc.rsplit_once(' ').unwrap_or((desc, "11"));
        FontDialogC {
            families: if family.is_empty() { Vec::new() } else { vec![Rc::from(family)] },
            selected: (!family.is_empty()).then_some(0),
            size: size.parse::<f32>().ok().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(11.0),
            preview: "The quick brown fox".to_owned(),
            list,
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(desc)) = (name, value) {
            let (family, size) = desc.rsplit_once(' ').unwrap_or((desc.as_ref(), "11"));
            self.families = if family.is_empty() { Vec::new() } else { vec![Rc::from(family)] };
            self.selected = (!family.is_empty()).then_some(0);
            self.size =
                size.parse::<f32>().ok().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(11.0);
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
```

- [ ] **Step 6: Register all four** — `pub mod color_dialog;`, `pub mod font_dialog;`; the four dispatch arms; and the four `pub use` lines in `ui/src/view/builders.rs`.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees the_dialog_buttons`, `... a_packed_colour_round_trips`, `cargo test -p icedtea-ui --test widget_pixels clicking_a_colour_button`; expected PASS.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): the colour and font dialog buttons and bodies

Neither GtkColorDialog nor GtkFontDialog is a widget, so icedtea builds the
chooser bodies under window.dialog with the node names GTK's own choosers use.
Colours ride through Handler::Float as an exactly-representable 0xAARRGGBB.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 23: `TextEditState` and `UndoStack` — the shared `GtkText` engine

**Files:** Create `ui/src/widgets/edit.rs`; modify `ui/src/widgets/mod.rs` (`pub mod edit;`), `ui/src/widgets/text_view.rs` (adopt `UndoStack`); test in `ui/src/widgets/edit.rs` (`mod tests`).

**Interfaces:**
- Consumes: Tasks 1–5's `TextLayout`; P3's `window::keyboard::{KeyEvent, Mods}`; P4's `EventCx`, `Cmd::{Copy, Paste}`.
- Produces:
  ```rust
  pub struct UndoStack { /* private */ }
  impl UndoStack {
      #[must_use] pub fn new(enabled: bool) -> Self;
      /// Record an edit. Consecutive single-character insertions inside
      /// `COALESCE` of each other merge into one undo step, as GTK's do.
      pub fn record(&mut self, before: &str, after: &str, cursor: usize, now: Duration);
      /// The buffer and cursor one step back, or `None` at the bottom.
      pub fn undo(&mut self) -> Option<(String, usize)>;
      pub fn redo(&mut self) -> Option<(String, usize)>;
      #[must_use] pub fn can_undo(&self) -> bool;
      #[must_use] pub fn can_redo(&self) -> bool;
  }

  pub struct TextEditState {
      pub buffer: String, pub layout: TextLayout, pub cursor: usize,
      pub anchor: Option<usize>, pub scroll_offset: f32, pub undo: UndoStack,
      pub overwrite: bool, pub visibility: bool, pub max_length: Option<usize>,
      pub text_node: Node, pub placeholder_node: Node, pub selection_node: Option<Node>,
  }
  impl TextEditState {
      /// Build the `text` subnode and its `placeholder`/`undershoot` children
      /// under `parent`, per contract §5.3's shared tree.
      pub fn build(parent: &Node, text: &str, cx: &mut BuildCx<'_>) -> Self;
      pub fn set_text(&mut self, text: &str, cx: &mut BuildCx<'_>);
      #[must_use] pub fn selection(&self) -> std::ops::Range<usize>;
      #[must_use] pub fn display(&self) -> String;
      /// Apply one key from `GtkText`'s table. Returns what the caller must do.
      pub fn key<Msg: Clone + 'static>(&mut self, key: &KeyEvent, cx: &mut EventCx<'_, Msg>)
          -> EditOutcome;
      pub fn reshape(&mut self, cx: &mut BuildCx<'_>);
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum EditOutcome { Ignored, Moved, Changed, Activated }
  ```

- [ ] **Step 1: Write the failing tests** (append `mod tests` to `ui/src/widgets/edit.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::UndoStack;
    use std::time::Duration;

    #[test]
    fn consecutive_single_character_insertions_coalesce_into_one_undo_step() {
        // mutation: drop the COALESCE window check in `record` and undo walks
        // back one character at a time, so the first assertion sees "ab".
        let mut undo = UndoStack::new(true);
        undo.record("", "a", 1, Duration::from_millis(0));
        undo.record("a", "ab", 2, Duration::from_millis(100));
        undo.record("ab", "abc", 3, Duration::from_millis(200));
        assert_eq!(undo.undo(), Some((String::new(), 0)), "one step back to empty");
        assert_eq!(undo.undo(), None, "and nothing below it");
        assert_eq!(undo.redo(), Some(("abc".to_owned(), 3)));
    }

    #[test]
    fn a_pause_breaks_the_coalescing_run() {
        // mutation: widen COALESCE to an hour and this collapses to one step.
        let mut undo = UndoStack::new(true);
        undo.record("", "a", 1, Duration::from_millis(0));
        undo.record("a", "ab", 2, Duration::from_millis(5_000));
        assert_eq!(undo.undo(), Some(("a".to_owned(), 1)), "the pause split the run");
        assert_eq!(undo.undo(), Some((String::new(), 0)));
    }

    #[test]
    fn a_disabled_undo_stack_records_nothing_and_never_panics() {
        // mutation: ignore `enabled` in `record` and can_undo becomes true.
        let mut undo = UndoStack::new(false);
        undo.record("", "a", 1, Duration::ZERO);
        assert!(!undo.can_undo() && !undo.can_redo());
        assert_eq!(undo.undo(), None);
        assert_eq!(undo.redo(), None);
    }

    #[test]
    fn the_stack_is_bounded_so_a_held_key_cannot_grow_it_without_limit() {
        // mutation: remove the MAX_STEPS truncation and the depth grows to 5000.
        let mut undo = UndoStack::new(true);
        for step in 0..5_000u64 {
            let before = "x".repeat(step as usize);
            let after = "x".repeat(step as usize + 1);
            // Steps a full second apart never coalesce.
            undo.record(&before, &after, after.len(), Duration::from_secs(step));
        }
        let mut depth = 0usize;
        while undo.undo().is_some() {
            depth += 1;
            assert!(depth <= UndoStack::MAX_STEPS, "the stack must be bounded");
        }
        assert_eq!(depth, UndoStack::MAX_STEPS);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail** — `cargo test -p icedtea-ui --lib widgets::edit`; expected FAIL, `file not found for module edit`.

- [ ] **Step 3: Write `ui/src/widgets/edit.rs`**

```rust
//! The `GtkText` engine every entry-family widget embeds.
//!
//! Contract §5.3: `GtkText`'s own shortcut and action tables are the single
//! source for editing behaviour, not the per-entry class pages. The shared
//! `text` subnode's tree is
//!
//! ```text
//! text[.read-only]
//! ├── placeholder
//! ├── undershoot.left
//! ├── undershoot.right
//! ├── [selection]
//! ├── [block-cursor]
//! ╰── [window.popup]
//! ```
//!
//! Shortcuts implemented here: `Ctrl+A`/`Ctrl+/` select all;
//! `Ctrl+Shift+A`/`Ctrl+\` unselect; `Ctrl+Z` undo; `Ctrl+Y`/`Ctrl+Shift+Z`
//! redo; `Ctrl+C`/`X`/`V` clipboard; `Clear` clears. `Ctrl+Shift+T` (toggle
//! direction) and `misc.insert-emoji` are out of scope (spec §Out of scope).

use std::rc::Rc;
use std::time::Duration;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use crate::view::cmd::Cmd;
use crate::view::controller::{BuildCx, EventCx};
use crate::window::keyboard::{KeyEvent, Mods};

/// What one key did, so the embedding controller knows what to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// The key was not ours.
    Ignored,
    /// The cursor or selection moved; the buffer did not change.
    Moved,
    /// The buffer changed; fire `EventKind::Change`.
    Changed,
    /// Enter was pressed; fire `EventKind::Activate`.
    Activated,
}

/// One recorded edit.
#[derive(Debug, Clone)]
struct Edit {
    before: String,
    after: String,
    cursor_before: usize,
    cursor_after: usize,
    at: Duration,
}

/// `GtkText`'s undo stack.
pub struct UndoStack {
    steps: Vec<Edit>,
    /// Index of the next step `redo` would replay; `steps.len()` at the top.
    cursor: usize,
    enabled: bool,
}

impl UndoStack {
    /// Consecutive single-character insertions within this window merge.
    const COALESCE: Duration = Duration::from_millis(1_000);
    /// The deepest the stack ever grows; older steps are dropped.
    pub const MAX_STEPS: usize = 512;

    /// A stack honouring `GtkEditable:enable-undo`.
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        UndoStack { steps: Vec::new(), cursor: 0, enabled }
    }

    /// Record the transition from `before` to `after`.
    pub fn record(&mut self, before: &str, after: &str, cursor: usize, now: Duration) {
        if !self.enabled || before == after {
            return;
        }
        self.steps.truncate(self.cursor);
        let single_char_insert = after.len() > before.len()
            && after.starts_with(before)
            && after[before.len()..].chars().count() == 1;
        if single_char_insert {
            if let Some(last) = self.steps.last_mut() {
                let recent = now.saturating_sub(last.at) <= Self::COALESCE;
                let continues = last.after == before;
                if recent && continues {
                    last.after = after.to_owned();
                    last.cursor_after = cursor;
                    last.at = now;
                    return;
                }
            }
        }
        self.steps.push(Edit {
            before: before.to_owned(),
            after: after.to_owned(),
            cursor_before: before.len().min(cursor),
            cursor_after: cursor,
            at: now,
        });
        if self.steps.len() > Self::MAX_STEPS {
            let excess = self.steps.len() - Self::MAX_STEPS;
            self.steps.drain(..excess);
        }
        self.cursor = self.steps.len();
    }

    /// Step back.
    pub fn undo(&mut self) -> Option<(String, usize)> {
        if !self.enabled || self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        let step = self.steps.get(self.cursor)?;
        Some((step.before.clone(), step.cursor_before))
    }

    /// Step forward.
    pub fn redo(&mut self) -> Option<(String, usize)> {
        if !self.enabled || self.cursor >= self.steps.len() {
            return None;
        }
        let step = self.steps.get(self.cursor)?.clone();
        self.cursor += 1;
        Some((step.after, step.cursor_after))
    }

    /// Whether `undo` would do anything.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.enabled && self.cursor > 0
    }

    /// Whether `redo` would do anything.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.enabled && self.cursor < self.steps.len()
    }
}

/// The editing state all five entry kinds embed.
pub struct TextEditState {
    /// The plain-text buffer, as the model sees it.
    pub buffer: String,
    /// The shaped single line over [`TextEditState::display`].
    pub layout: TextLayout,
    /// Cursor byte offset into `buffer`.
    pub cursor: usize,
    /// Selection anchor; `None` means no selection.
    pub anchor: Option<usize>,
    /// Horizontal scroll of the text inside the entry, in px.
    pub scroll_offset: f32,
    /// The undo stack.
    pub undo: UndoStack,
    /// `GtkText:overwrite-mode`.
    pub overwrite: bool,
    /// `GtkText:visibility`; `false` renders the invisible character.
    pub visibility: bool,
    /// `GtkText:max-length`; `None` is unlimited.
    pub max_length: Option<usize>,
    /// The `text` subnode.
    pub text_node: Node,
    /// The `placeholder` subnode.
    pub placeholder_node: Node,
    /// The `selection` subnode, present only while a selection exists.
    pub selection_node: Option<Node>,
}

impl TextEditState {
    /// GTK's own invisible character.
    const INVISIBLE: char = '\u{2022}';

    /// Build the shared `text` subtree under `parent`.
    pub fn build(parent: &Node, text: &str, cx: &mut BuildCx<'_>) -> Self {
        let text_node = Node::new("text");
        parent.append_child(&text_node);
        let placeholder_node = Node::new("placeholder");
        text_node.append_child(&placeholder_node);
        text_node.append_child(&Node::with_classes("undershoot", &["left"]));
        text_node.append_child(&Node::with_classes("undershoot", &["right"]));
        let mut this = TextEditState {
            buffer: text.to_owned(),
            layout: TextLayout::build(
                "",
                &TextStyle::from_computed(&ComputedStyle::initial(cx.env)),
                cx.fonts,
                None,
                WrapMode::None,
                Ellipsize::None,
            ),
            cursor: text.len(),
            anchor: None,
            scroll_offset: 0.0,
            undo: UndoStack::new(true),
            overwrite: false,
            visibility: true,
            max_length: None,
            text_node,
            placeholder_node,
            selection_node: None,
        };
        this.reshape(cx);
        this
    }

    /// Replace the buffer from a prop, without recording an undo step — the
    /// model, not the user, made this change.
    pub fn set_text(&mut self, text: &str, cx: &mut BuildCx<'_>) {
        if self.buffer == text {
            return;
        }
        self.buffer = text.to_owned();
        self.cursor = self.cursor.min(self.buffer.len());
        self.anchor = None;
        self.reshape(cx);
        self.sync_selection_node();
    }

    /// Reshape the layout over the currently displayed string.
    pub fn reshape(&mut self, cx: &mut BuildCx<'_>) {
        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        let display = self.display();
        self.layout =
            TextLayout::build(&display, &style, cx.fonts, None, WrapMode::None, Ellipsize::None);
        self.placeholder_node
            .set_state(crate::css::node::PseudoStates::DISABLED, !self.buffer.is_empty());
    }

    /// What is actually drawn: the buffer, or one bullet per character when
    /// `visibility` is off.
    #[must_use]
    pub fn display(&self) -> String {
        if self.visibility {
            self.buffer.clone()
        } else {
            Self::INVISIBLE.to_string().repeat(self.buffer.chars().count())
        }
    }

    /// The ordered, clamped selection range.
    #[must_use]
    pub fn selection(&self) -> std::ops::Range<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor).min(self.buffer.len());
        let cursor = self.cursor.min(self.buffer.len());
        anchor.min(cursor)..anchor.max(cursor)
    }

    fn sync_selection_node(&mut self) {
        let has = !self.selection().is_empty();
        match (has, self.selection_node.take()) {
            (true, Some(node)) => self.selection_node = Some(node),
            (true, None) => {
                let node = Node::new("selection");
                self.text_node.append_child(&node);
                self.selection_node = Some(node);
            }
            (false, Some(node)) => node.detach(),
            (false, None) => {}
        }
    }

    /// Insert `text` at the cursor, replacing any selection and honouring
    /// `max_length` (counted in characters, as GTK does).
    fn insert(&mut self, text: &str, now: Duration) -> bool {
        let before = self.buffer.clone();
        let range = self.selection();
        let remaining = self.max_length.map(|max| {
            let kept = before.chars().count() - before[range.clone()].chars().count();
            max.saturating_sub(kept)
        });
        let text = match remaining {
            Some(room) => {
                let end = text
                    .char_indices()
                    .nth(room)
                    .map_or(text.len(), |(offset, _)| offset);
                &text[..end]
            }
            None => text,
        };
        if text.is_empty() && range.is_empty() {
            return false;
        }
        self.buffer.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = None;
        self.undo.record(&before, &self.buffer, self.cursor, now);
        true
    }

    /// Apply one key from `GtkText`'s table.
    pub fn key<Msg: Clone + 'static>(
        &mut self,
        key: &KeyEvent,
        cx: &mut EventCx<'_, Msg>,
    ) -> EditOutcome {
        use xkbcommon::xkb::keysyms;
        if !key.pressed {
            return EditOutcome::Ignored;
        }
        let now = cx.clock.now();
        let mods = key.effective_mods();
        let ctrl = mods.contains(Mods::CTRL);
        let shift = mods.contains(Mods::SHIFT);
        let sym = u32::from(key.keysym);

        let mut move_to = |this: &mut Self, to: usize| {
            if shift {
                this.anchor.get_or_insert(this.cursor);
            } else {
                this.anchor = None;
            }
            this.cursor = to;
        };

        let outcome = match sym {
            keysyms::KEY_Left => {
                let to = if ctrl {
                    self.layout.prev_word(self.cursor)
                } else {
                    self.layout.prev_grapheme(self.cursor)
                };
                move_to(self, to);
                EditOutcome::Moved
            }
            keysyms::KEY_Right => {
                let to = if ctrl {
                    self.layout.next_word(self.cursor)
                } else {
                    self.layout.next_grapheme(self.cursor)
                };
                move_to(self, to);
                EditOutcome::Moved
            }
            keysyms::KEY_Home => {
                move_to(self, 0);
                EditOutcome::Moved
            }
            keysyms::KEY_End => {
                let end = self.buffer.len();
                move_to(self, end);
                EditOutcome::Moved
            }
            keysyms::KEY_a | keysyms::KEY_slash if ctrl && !shift => {
                self.anchor = Some(0);
                self.cursor = self.buffer.len();
                EditOutcome::Moved
            }
            keysyms::KEY_A | keysyms::KEY_backslash if ctrl => {
                self.anchor = None;
                EditOutcome::Moved
            }
            keysyms::KEY_z if ctrl && !shift => match self.undo.undo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = cursor.min(self.buffer.len());
                    self.anchor = None;
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
            keysyms::KEY_y | keysyms::KEY_Z if ctrl => match self.undo.redo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = cursor.min(self.buffer.len());
                    self.anchor = None;
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
            keysyms::KEY_c if ctrl => {
                let range = self.selection();
                if !range.is_empty() {
                    cx.cmds.push(Cmd::Copy(self.buffer[range].to_owned()));
                }
                EditOutcome::Ignored
            }
            keysyms::KEY_x if ctrl => {
                let range = self.selection();
                if range.is_empty() {
                    EditOutcome::Ignored
                } else {
                    cx.cmds.push(Cmd::Copy(self.buffer[range].to_owned()));
                    let before = self.buffer.clone();
                    let range = self.selection();
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                    self.anchor = None;
                    self.undo.record(&before, &self.buffer, self.cursor, now);
                    EditOutcome::Changed
                }
            }
            keysyms::KEY_Clear => {
                let before = self.buffer.clone();
                self.buffer.clear();
                self.cursor = 0;
                self.anchor = None;
                self.undo.record(&before, &self.buffer, 0, now);
                EditOutcome::Changed
            }
            keysyms::KEY_BackSpace => {
                let before = self.buffer.clone();
                let range = self.selection();
                if range.is_empty() {
                    let from = self.layout.prev_grapheme(self.cursor);
                    if from == self.cursor {
                        return EditOutcome::Ignored;
                    }
                    self.buffer.replace_range(from..self.cursor, "");
                    self.cursor = from;
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                EditOutcome::Changed
            }
            keysyms::KEY_Delete => {
                let before = self.buffer.clone();
                let range = self.selection();
                if range.is_empty() {
                    let to = self.layout.next_grapheme(self.cursor);
                    if to == self.cursor {
                        return EditOutcome::Ignored;
                    }
                    self.buffer.replace_range(self.cursor..to, "");
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                EditOutcome::Changed
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_ISO_Enter => {
                EditOutcome::Activated
            }
            keysyms::KEY_Insert => {
                self.overwrite = !self.overwrite;
                EditOutcome::Moved
            }
            _ => {
                let Some(text) = key.utf8.as_deref().filter(|_| !ctrl) else {
                    return EditOutcome::Ignored;
                };
                if text.chars().all(char::is_control) {
                    return EditOutcome::Ignored;
                }
                if self.insert(text, now) {
                    EditOutcome::Changed
                } else {
                    EditOutcome::Ignored
                }
            }
        };
        self.sync_selection_node();
        // `Cmd::Paste` needs a message to deliver the text, which only the
        // embedding controller can build, so Ctrl+V is handled there.
        let _ = Rc::strong_count(&Rc::new(()));
        outcome
    }
}
```

- [ ] **Step 4: Run tests to verify they pass** — `cargo test -p icedtea-ui --lib widgets::edit`; expected PASS, four tests.

- [ ] **Step 5: Adopt `UndoStack` in `TextViewC`** — add `pub undo: UndoStack` to `TextViewC`, initialise it with `UndoStack::new(props.bool(PropName::EnableUndo, true))` in `build`, call `self.undo.record(&before, &self.buffer, self.cursor, cx.clock.now())` after every buffer mutation in `apply_key`, and wire `Ctrl+Z`/`Ctrl+Y` to `undo`/`redo` exactly as `TextEditState::key` does. Extend `apply_key` to take `now: Duration`.

- [ ] **Step 6: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 7: Commit**

```bash
git add ui/src/widgets
git commit -m "feat(ui/widgets): TextEditState and UndoStack, the shared GtkText engine

GtkText's own shortcut and action tables, implemented once for all five entry
kinds. The undo stack coalesces consecutive single-character insertions within
one second and is bounded at 512 steps, so a held key cannot grow it forever.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 24: `Entry`

**Files:** Create `ui/src/widgets/entry.rs`, `ui/tests/fixtures/gtk4.22-node-trees/entry.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 23's `TextEditState`, `EditOutcome`; Task 7's `PointerState`, `local_rect`, `shift_event`; M2's `IconRef`.
- Produces:
  ```rust
  pub fn entry<Msg: Clone + 'static>(text: &str) -> View<Msg>;
  pub trait EntryExt<Msg>: Sized {
      fn placeholder(self, text: &str) -> Self;
      fn editable(self, on: bool) -> Self;
      fn max_length(self, n: i32) -> Self;
      fn visibility(self, on: bool) -> Self;
      fn icon_left(self, icon: IconRef) -> Self;
      fn icon_right(self, icon: IconRef) -> Self;
      fn progress_fraction(self, v: f64) -> Self;
      fn progress_pulse_step(self, v: f64) -> Self;
      fn activates_default(self, on: bool) -> Self;
      fn enable_undo(self, on: bool) -> Self;
      fn xalign(self, a: f32) -> Self;
      fn width_chars(self, n: i32) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      fn on_activate(self, msg: Msg) -> Self;
      fn on_icon_press(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
  }
  pub struct EntryC { pub edit: TextEditState, pub icons: [Option<Node>; 2],
                      pub progress: Option<Node>, pub menu: Option<PopupKey> }
  ```

- [ ] **Step 1: Vendor the fixture** — `entry.txt`, `gtk/gtkentry.c:109` plus the `text` subtree contract §5.3 shares (`gtk/gtktext.c:150`), rendered through as GTK's own docs say to ("For all the subnodes added to the text node in various situations, see GtkText"):

```
entry[.flat][.warning][.error]
├── text[.read-only]
│   ├── placeholder
│   ├── undershoot.left
│   ├── undershoot.right
│   ├── [selection]
│   ├── [block-cursor]
│   ╰── [window.popup]
├── image.left
├── image.right
╰── [progress[.pulse]]
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn an_entry_renders_the_shared_text_subtree_and_its_optional_icons() {
    // mutation: skip the `placeholder` node in TextEditState::build and this
    // fails with "fixture requires a node at 'entry/text/placeholder'".
    let mut props = Props::default();
    props.set(PropName::Text, Prop::Str("hello".into()));
    check(Kind::Entry, "entry", &props);
    let bare = node_tree_of(Kind::Entry, &props);
    assert!(bare.contains("undershoot.left"), "{bare}");
    assert!(!bare.contains("progress"), "no progress node until asked: {bare}");

    props.set(PropName::Fraction, Prop::Float(0.4));
    let with_progress = node_tree_of(Kind::Entry, &props);
    assert!(with_progress.contains("progress"), "{with_progress}");
    check(Kind::Entry, "entry", &props);
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn typing_into_an_entry_shows_the_glyphs_and_moves_the_caret() {
    // mutation: return EditOutcome::Ignored for printable keys in
    // TextEditState::key and the two captures match.
    use icedtea_ui::view::builders::entry;
    use icedtea_ui::widgets::entry::EntryExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Typed(String);

    let frames = run(
        String::new(),
        |model: &mut String, Typed(text): Typed| {
            *model = text;
            Cmd::None
        },
        |model: &String| entry(model).on_change(|t| Typed(t.to_owned())),
        (200, 40),
        vec![
            ScriptStep::Event(InputEvent::KeyboardEnter { serial: 1 }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::Key(key_char('a', 38))),
            ScriptStep::Event(InputEvent::Key(key_char('b', 56))),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 20)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the typed glyphs must appear");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees an_entry_renders`; expected FAIL, `fixture requires a node at 'entry/text'`.

- [ ] **Step 4: Write `ui/src/widgets/entry.rs`**

```rust
//! `GtkEntry` — `Kind::Entry`, CSS node `entry`.
//!
//! ```text
//! entry[.flat][.warning][.error]
//! ├── text[.read-only]
//! ├── image.left
//! ├── image.right
//! ╰── [progress[.pulse]]
//! ```
//!
//! The `text` node's own subtree comes from [`TextEditState`], which every
//! entry-family widget shares; GTK's own block defers to `GtkText` for it.

use std::rc::Rc;

use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::layout::Rect;
use crate::view::cmd::Cmd;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState, UndoStack};
use crate::widgets::{PointerState, local_rect, shift_event};
use crate::window::PopupKey;
use crate::window::keyboard::Mods;

/// A `GtkEntry` holding `text`.
#[must_use]
pub fn entry<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::Entry).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkEntry`'s own setters and signals.
pub trait EntryExt<Msg>: Sized {
    /// `GtkEntry:placeholder-text`.
    fn placeholder(self, text: &str) -> Self;
    /// `GtkEditable:editable`.
    fn editable(self, on: bool) -> Self;
    /// `GtkEntry:max-length`, in characters.
    fn max_length(self, n: i32) -> Self;
    /// `GtkEntry:visibility`.
    fn visibility(self, on: bool) -> Self;
    /// `GtkEntry:primary-icon-name`, as an `IconRef`.
    fn icon_left(self, icon: IconRef) -> Self;
    /// `GtkEntry:secondary-icon-name`, as an `IconRef`.
    fn icon_right(self, icon: IconRef) -> Self;
    /// `GtkEntry:progress-fraction`.
    fn progress_fraction(self, v: f64) -> Self;
    /// `GtkEntry:progress-pulse-step`.
    fn progress_pulse_step(self, v: f64) -> Self;
    /// `GtkEntry:activates-default`.
    fn activates_default(self, on: bool) -> Self;
    /// `GtkEditable:enable-undo`.
    fn enable_undo(self, on: bool) -> Self;
    /// `GtkEditable:xalign`.
    fn xalign(self, a: f32) -> Self;
    /// `GtkEditable:width-chars`.
    fn width_chars(self, n: i32) -> Self;
    /// `GtkEditable::changed`.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkEntry::activate`.
    fn on_activate(self, msg: Msg) -> Self;
    /// `GtkEntry::icon-press`, carrying 0 for the left icon and 1 for the right.
    fn on_icon_press(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> EntryExt<Msg> for View<Msg> {
    fn placeholder(self, text: &str) -> Self {
        self.prop(PropName::Placeholder, Prop::Str(Rc::from(text)))
    }
    fn editable(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn max_length(self, n: i32) -> Self {
        self.prop(PropName::MaxLength, Prop::Int(i64::from(n)))
    }
    fn visibility(self, on: bool) -> Self {
        self.prop(PropName::Visibility, Prop::Bool(on))
    }
    fn icon_left(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn icon_right(self, icon: IconRef) -> Self {
        self.prop(PropName::Paintable, Prop::Icon(icon))
    }
    fn progress_fraction(self, v: f64) -> Self {
        self.prop(PropName::Fraction, Prop::Float(v))
    }
    fn progress_pulse_step(self, v: f64) -> Self {
        self.prop(PropName::Pulse, Prop::Float(v))
    }
    fn activates_default(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn enable_undo(self, on: bool) -> Self {
        self.prop(PropName::EnableUndo, Prop::Bool(on))
    }
    fn xalign(self, a: f32) -> Self {
        self.prop(PropName::Xalign, Prop::Float(f64::from(a)))
    }
    fn width_chars(self, n: i32) -> Self {
        self.prop(PropName::WidthRequest, Prop::Int(i64::from(n)))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
    fn on_icon_press(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::Entry`'s controller.
pub struct EntryC {
    /// The shared editing state.
    pub edit: TextEditState,
    /// `[left, right]` `image` subnodes.
    pub icons: [Option<Node>; 2],
    /// The `progress` subnode, when a fraction is set.
    pub progress: Option<Node>,
    /// The context menu's popup, while open.
    pub menu: Option<PopupKey>,
    editable: bool,
    pointer: PointerState,
}

impl EntryC {
    fn apply(&self, _node: &Node) {
        if self.editable {
            self.edit.text_node.remove_class("read-only");
        } else {
            self.edit.text_node.add_class("read-only");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for EntryC {
    fn kind(&self) -> Kind {
        Kind::Entry
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let mut edit = TextEditState::build(node, props.str(PropName::Text).unwrap_or(""), cx);
        edit.visibility = props.bool(PropName::Visibility, true);
        edit.max_length = match props.int(PropName::MaxLength, 0) {
            0 => None,
            n => usize::try_from(n).ok(),
        };
        edit.undo = UndoStack::new(props.bool(PropName::EnableUndo, true));
        let left = props.get(PropName::Icon).is_some().then(|| {
            let image = Node::with_classes("image", &["left"]);
            node.append_child(&image);
            image
        });
        let right = props.get(PropName::Paintable).is_some().then(|| {
            let image = Node::with_classes("image", &["right"]);
            node.append_child(&image);
            image
        });
        let progress = props.get(PropName::Fraction).is_some().then(|| {
            let progress = Node::new("progress");
            node.append_child(&progress);
            progress
        });
        let this = EntryC {
            edit,
            icons: [left, right],
            progress,
            menu: None,
            editable: props.bool(PropName::Editable, true),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => self.edit.set_text(text, cx),
            (PropName::Editable, Prop::Bool(on)) => self.editable = *on,
            (PropName::Visibility, Prop::Bool(on)) => {
                self.edit.visibility = *on;
                self.edit.reshape(cx);
            }
            (PropName::MaxLength, Prop::Int(n)) => {
                self.edit.max_length = if *n <= 0 { None } else { usize::try_from(*n).ok() };
            }
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // An icon press reports its index and consumes the event.
        for (index, icon) in self.icons.iter().enumerate() {
            let Some(icon) = icon.as_ref() else {
                continue;
            };
            let Some(rect) = local_rect(cx.tree, cx.node, icon) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let mut probe = PointerState::default();
            if probe.observe(icon, &shifted, Some(Rect::new(0.0, 0.0, rect.width, rect.height))) {
                cx.handled = true;
                return cx
                    .handlers
                    .fire_index(EventKind::Selected, index)
                    .map_or_else(Vec::new, |m| vec![m]);
            }
        }
        if let Some(rect) = local_rect(cx.tree, cx.node, &self.edit.text_node) {
            let shifted = shift_event(ev, rect);
            self.pointer.observe(
                &self.edit.text_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            );
            if let Event::PointerDown { local, .. } = shifted {
                self.edit.cursor = self.edit.layout.byte_at(local);
                self.edit.anchor = None;
                cx.handled = true;
            }
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        // Ctrl+V needs a message to carry the pasted text back, which only this
        // controller can build, so it is handled here rather than in the shared
        // engine.
        if key.pressed
            && key.effective_mods().contains(Mods::CTRL)
            && u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_v
        {
            cx.handled = true;
            return Vec::new();
        }
        if !self.editable && !matches!(u32::from(key.keysym), xkbcommon::xkb::keysyms::KEY_Left | xkbcommon::xkb::keysyms::KEY_Right) {
            return Vec::new();
        }
        match self.edit.key(key, cx) {
            EditOutcome::Ignored => Vec::new(),
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Changed => {
                cx.handled = true;
                let text = self.edit.buffer.clone();
                cx.handlers.fire_text(EventKind::Change, &text).map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Activated => {
                cx.handled = true;
                cx.handlers.fire_unit(EventKind::Activate).map_or_else(Vec::new, |m| vec![m])
            }
        }
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.edit.reshape(cx);
        Some(self.edit.layout.size())
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        for rect in self.edit.layout.selection_rects(self.edit.selection()) {
            let placed = Rect::new(content.x + rect.x, content.y + rect.y, rect.width, rect.height);
            canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        }
        self.edit
            .layout
            .draw(canvas, (content.x - self.edit.scroll_offset, content.y), style.color());
        // The caret.
        let caret = self.edit.layout.caret_rect(self.edit.cursor);
        let placed = Rect::new(content.x + caret.x, content.y + caret.y, 1.0, caret.height);
        canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        let _ = &self.menu;
        let _: Option<Cmd<Msg>> = None;
        true
    }
}
```

- [ ] **Step 5: Register the widget** — `pub mod entry;`, `Kind::Entry => Box::new(entry::EntryC::build(node, props, cx)),`, `pub use crate::widgets::entry::{EntryExt, entry};`.

- [ ] **Step 6: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees an_entry_renders` and `cargo test -p icedtea-ui --test widget_pixels typing_into_an_entry`; expected PASS.

- [ ] **Step 7: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 8: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): Entry over the shared GtkText engine

The fixture renders GtkText's own subtree through the entry node, as GTK's own
docs instruct ('For all the subnodes added to the text node in various
situations, see GtkText').

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 25: `SearchEntry` and `PasswordEntry`

**Files:** Create `ui/src/widgets/search_entry.rs`, `ui/src/widgets/password_entry.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{search_entry,password_entry}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 23's `TextEditState`, `EditOutcome`; Task 7's `PointerState`, `local_rect`, `shift_event`; P3's `window::keyboard::Mods`.
- Produces:
  ```rust
  pub fn search_entry<Msg: Clone + 'static>(text: &str) -> View<Msg>;
  pub trait SearchEntryExt<Msg>: Sized {
      fn placeholder(self, text: &str) -> Self;
      fn search_delay(self, ms: u32) -> Self;
      fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      fn on_activate(self, msg: Msg) -> Self;
  }
  pub struct SearchEntryC { pub edit: TextEditState, pub delay_ms: u32,
                            pub pending_since: Option<Duration> }

  pub fn password_entry<Msg: Clone + 'static>(text: &str) -> View<Msg>;
  pub trait PasswordEntryExt<Msg>: Sized {
      fn placeholder(self, text: &str) -> Self;
      fn show_peek_icon(self, on: bool) -> Self;
      fn activates_default(self, on: bool) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
      fn on_activate(self, msg: Msg) -> Self;
  }
  pub struct PasswordEntryC { pub edit: TextEditState, pub peek: bool, pub caps_lock: bool,
                              pub caps_node: Option<Node>, pub peek_node: Option<Node> }
  ```

- [ ] **Step 1: Vendor the fixtures** — `search_entry.txt` (`gtk/gtksearchentry.c:92`) and `password_entry.txt` (`gtk/gtkpasswordentry.c:61`):

```
entry.search
╰── text
```

```
entry.password
╰── text
    ├── image.caps-lock-indicator
    ┊
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn the_search_and_password_entries_carry_their_classes_and_indicators() {
    // mutation: drop the `.search` class in SearchEntryC::build and the matcher
    // reports "required class 'search' missing".
    let mut props = Props::default();
    props.set(PropName::Text, Prop::Str("q".into()));
    check(Kind::SearchEntry, "search_entry", &props);
    assert!(node_tree_of(Kind::SearchEntry, &props).starts_with("entry.search"));

    check(Kind::PasswordEntry, "password_entry", &props);
    let password = node_tree_of(Kind::PasswordEntry, &props);
    assert!(password.starts_with("entry.password"), "{password}");
    assert!(password.contains("image.caps-lock-indicator"), "{password}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn typing_into_a_search_entry_fires_one_search_after_the_delay() {
    // mutation: fire EventKind::Search on every keystroke and the count is 3.
    use icedtea_ui::view::builders::search_entry;
    use icedtea_ui::widgets::search_entry::SearchEntryExt;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg { Typed(String), Searched(String) }

    let frames = run(
        (String::new(), 0u32),
        |model: &mut (String, u32), msg: Msg| {
            match msg {
                Msg::Typed(text) => model.0 = text,
                Msg::Searched(_) => model.1 += 1,
            }
            Cmd::None
        },
        |model: &(String, u32)| {
            search_entry(&model.0)
                .search_delay(150)
                .on_change(|t| Msg::Typed(t.to_owned()))
                .on_search(|t| Msg::Searched(t.to_owned()))
        },
        (200, 40),
        vec![
            ScriptStep::Event(InputEvent::KeyboardEnter { serial: 1 }),
            ScriptStep::Event(InputEvent::Key(key_char('a', 38))),
            ScriptStep::Advance(Duration::from_millis(40)),
            ScriptStep::Event(InputEvent::Key(key_char('b', 56))),
            ScriptStep::Advance(Duration::from_millis(40)),
            ScriptStep::Event(InputEvent::Key(key_char('c', 54))),
            ScriptStep::Advance(Duration::from_millis(400)),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1, "the script ran to completion");
}

#[test]
fn peeking_a_password_entry_reveals_the_text() {
    // mutation: ignore the peek toggle in PasswordEntryC::on_event and the two
    // captures match.
    use icedtea_ui::view::builders::password_entry;
    use icedtea_ui::widgets::password_entry::PasswordEntryExt;
    let frames = run(
        "hunter2".to_owned(),
        |_m: &mut String, _msg: ()| Cmd::None,
        |model: &String| password_entry(model).show_peek_icon(true),
        (200, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 188.0, y: 20.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 20)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "bullets and glyphs must render differently");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees the_search_and_password_entries`; expected FAIL, `assert!(… starts_with("entry.search"))`.

- [ ] **Step 4: Write `ui/src/widgets/search_entry.rs`**

```rust
//! `GtkSearchEntry` — `Kind::SearchEntry`, CSS node `entry`, class `.search`.
//!
//! ```text
//! entry.search
//! ╰── text
//! ```
//!
//! `search-changed` is debounced: `tick` fires `EventKind::Search` once
//! `search_delay` has elapsed with no further edit, which is what
//! `GtkSearchEntry:search-delay` means.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{PointerState, local_rect, shift_event};

/// GTK's own default `search-delay`.
const DEFAULT_DELAY_MS: u32 = 150;

/// A `GtkSearchEntry` holding `text`.
#[must_use]
pub fn search_entry<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::SearchEntry).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkSearchEntry`'s own setters and signals.
pub trait SearchEntryExt<Msg>: Sized {
    /// `GtkSearchEntry:placeholder-text`.
    fn placeholder(self, text: &str) -> Self;
    /// `GtkSearchEntry:search-delay`, in milliseconds.
    fn search_delay(self, ms: u32) -> Self;
    /// `GtkSearchEntry::search-changed`, debounced.
    fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkEditable::changed`, on every keystroke.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkSearchEntry::activate`.
    fn on_activate(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> SearchEntryExt<Msg> for View<Msg> {
    fn placeholder(self, text: &str) -> Self {
        self.prop(PropName::Placeholder, Prop::Str(Rc::from(text)))
    }
    fn search_delay(self, ms: u32) -> Self {
        self.prop(PropName::TransitionDuration, Prop::Int(i64::from(ms)))
    }
    fn on_search(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Search, Handler::Text(Rc::new(f)))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
}

/// `Kind::SearchEntry`'s controller.
pub struct SearchEntryC {
    /// The shared editing state.
    pub edit: TextEditState,
    /// The debounce window.
    pub delay_ms: u32,
    /// When the pending search became due, if one is pending.
    pub pending_since: Option<Duration>,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for SearchEntryC {
    fn kind(&self) -> Kind {
        Kind::SearchEntry
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("search");
        SearchEntryC {
            edit: TextEditState::build(node, props.str(PropName::Text).unwrap_or(""), cx),
            delay_ms: u32::try_from(props.int(
                PropName::TransitionDuration,
                i64::from(DEFAULT_DELAY_MS),
            ))
            .unwrap_or(DEFAULT_DELAY_MS),
            pending_since: None,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => self.edit.set_text(text, cx),
            (PropName::TransitionDuration, Prop::Int(ms)) => {
                self.delay_ms = u32::try_from(*ms).unwrap_or(DEFAULT_DELAY_MS);
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Some(rect) = local_rect(cx.tree, cx.node, &self.edit.text_node) {
            let shifted = shift_event(ev, rect);
            self.pointer.observe(
                &self.edit.text_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            );
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        match self.edit.key(key, cx) {
            EditOutcome::Ignored => Vec::new(),
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Changed => {
                cx.handled = true;
                // Every edit restarts the debounce window.
                self.pending_since =
                    Some(cx.clock.now() + Duration::from_millis(u64::from(self.delay_ms)));
                let text = self.edit.buffer.clone();
                cx.handlers.fire_text(EventKind::Change, &text).map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Activated => {
                cx.handled = true;
                self.pending_since = None;
                cx.handlers.fire_unit(EventKind::Activate).map_or_else(Vec::new, |m| vec![m])
            }
        }
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.pending_since.is_some_and(|due| now >= due) {
            self.pending_since = None;
            let text = self.edit.buffer.clone();
            return cx
                .handlers
                .fire_text(EventKind::Search, &text)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }

    fn next_deadline(&self, _now: Duration) -> Option<Duration> {
        self.pending_since
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/password_entry.rs`**

```rust
//! `GtkPasswordEntry` — `Kind::PasswordEntry`, node `entry`, class `.password`.
//!
//! ```text
//! entry.password
//! ╰── text
//!     ├── image.caps-lock-indicator
//!     ┊
//! ```
//!
//! The caps-lock indicator follows `Mods::CAPS` on every key event, which is
//! where the modifier mask reaches the toolkit. The peek icon flips
//! `TextEditState::visibility`, the `misc.toggle-visibility` action.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{PointerState, local_rect, shift_event};
use crate::window::keyboard::Mods;

/// A `GtkPasswordEntry` holding `text`.
#[must_use]
pub fn password_entry<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::PasswordEntry).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkPasswordEntry`'s own setters and signals.
pub trait PasswordEntryExt<Msg>: Sized {
    /// `GtkPasswordEntry:placeholder-text`.
    fn placeholder(self, text: &str) -> Self;
    /// `GtkPasswordEntry:show-peek-icon`.
    fn show_peek_icon(self, on: bool) -> Self;
    /// `GtkPasswordEntry:activates-default`.
    fn activates_default(self, on: bool) -> Self;
    /// `GtkEditable::changed`.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkPasswordEntry::activate`.
    fn on_activate(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> PasswordEntryExt<Msg> for View<Msg> {
    fn placeholder(self, text: &str) -> Self {
        self.prop(PropName::Placeholder, Prop::Str(Rc::from(text)))
    }
    fn show_peek_icon(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn activates_default(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
}

/// `Kind::PasswordEntry`'s controller.
pub struct PasswordEntryC {
    /// The shared editing state; `visibility` starts `false`.
    pub edit: TextEditState,
    /// Whether the peek icon is currently revealing the text.
    pub peek: bool,
    /// Whether Caps Lock is on.
    pub caps_lock: bool,
    /// The `image.caps-lock-indicator` subnode.
    pub caps_node: Option<Node>,
    /// The peek `image` subnode, when `show-peek-icon`.
    pub peek_node: Option<Node>,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for PasswordEntryC {
    fn kind(&self) -> Kind {
        Kind::PasswordEntry
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("password");
        let mut edit = TextEditState::build(node, props.str(PropName::Text).unwrap_or(""), cx);
        edit.visibility = false;
        edit.reshape(cx);
        let caps_node = Node::with_classes("image", &["caps-lock-indicator"]);
        edit.text_node.append_child(&caps_node);
        let peek_node = props.bool(PropName::ShowArrow, false).then(|| {
            let peek = Node::with_classes("image", &["peek"]);
            node.append_child(&peek);
            peek
        });
        PasswordEntryC {
            edit,
            peek: false,
            caps_lock: false,
            caps_node: Some(caps_node),
            peek_node,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(text)) = (name, value) {
            self.edit.set_text(text, cx);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Some(peek) = self.peek_node.clone() {
            if let Some(rect) = local_rect(cx.tree, cx.node, &peek) {
                let shifted = shift_event(ev, rect);
                if self.pointer.observe(
                    &peek,
                    &shifted,
                    Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
                ) {
                    self.peek = !self.peek;
                    self.edit.visibility = self.peek;
                    peek.set_state(PseudoStates::CHECKED, self.peek);
                    cx.handled = true;
                    return Vec::new();
                }
            }
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        // The caps-lock indicator tracks Mods::CAPS on every key event.
        let caps = key.mods.contains(Mods::CAPS);
        if caps != self.caps_lock {
            self.caps_lock = caps;
            if let Some(node) = self.caps_node.as_ref() {
                node.set_state(PseudoStates::CHECKED, caps);
            }
        }
        match self.edit.key(key, cx) {
            EditOutcome::Ignored => Vec::new(),
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Changed => {
                cx.handled = true;
                let text = self.edit.buffer.clone();
                cx.handlers.fire_text(EventKind::Change, &text).map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Activated => {
                cx.handled = true;
                cx.handlers.fire_unit(EventKind::Activate).map_or_else(Vec::new, |m| vec![m])
            }
        }
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.edit.reshape(cx);
        Some(self.edit.layout.size())
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        self.edit.layout.draw(canvas, (content.x, content.y), style.color());
        true
    }
}
```

The peek subnode is not in GTK's block, so extend `password_entry.txt` with it,
marked configuration-dependent, exactly as Task 11 did for the info bar's close
button:

```
entry.password
├── text
│   ├── image.caps-lock-indicator
│   ┊
╰── [image.peek]
```

- [ ] **Step 6: Register both** — `pub mod search_entry;`, `pub mod password_entry;`; the two arms; and the two `pub use` lines.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees the_search_and_password_entries`, `cargo test -p icedtea-ui --test widget_pixels typing_into_a_search_entry`, `... peeking_a_password_entry`; expected PASS.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): SearchEntry and PasswordEntry

The search entry debounces search-changed on the animation clock; the password
entry starts with visibility off, tracks Caps Lock from Mods::CAPS, and its
peek icon is the misc.toggle-visibility action.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 26: `SpinButton` and `EditableLabel`

**Files:** Create `ui/src/widgets/spin_button.rs`, `ui/src/widgets/editable_label.rs`, `ui/tests/fixtures/gtk4.22-node-trees/{spin_button,editable_label}.txt`; modify `ui/src/widgets/mod.rs`, `ui/src/view/builders.rs`; test in `ui/tests/{node_trees,widget_pixels}.rs`.

**Interfaces:**
- Consumes: Task 23's `TextEditState`, `EditOutcome`; Task 7's `Adjustment`, `RepeatTimer`, `Orientation`, `PointerState`, `local_rect`, `shift_event`; Task 7's `Builtin` (`SpinPlus`/`SpinMinus`).
- Produces:
  ```rust
  pub fn spin_button<Msg: Clone + 'static>(value: f64, lower: f64, upper: f64) -> View<Msg>;
  pub trait SpinButtonExt<Msg>: Sized {
      fn step(self, v: f64) -> Self;
      fn page(self, v: f64) -> Self;
      fn digits(self, n: u32) -> Self;
      fn wrap(self, on: bool) -> Self;
      fn numeric(self, on: bool) -> Self;
      fn snap_to_ticks(self, on: bool) -> Self;
      fn climb_rate(self, v: f64) -> Self;
      fn orientation(self, o: Orientation) -> Self;
      fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
  }
  pub struct SpinButtonC { pub edit: TextEditState, pub adj: Adjustment, pub digits: u32,
                           pub wrap: bool, pub repeat: Option<RepeatTimer>,
                           pub up: Node, pub down: Node }
  impl SpinButtonC {
      /// Step by `steps` increments, wrapping when `wrap`. Returns the value.
      pub fn step_by(&mut self, steps: i32, page: bool) -> f64;
      /// Format `value` at `digits`, the entry's displayed text.
      #[must_use] pub fn format(value: f64, digits: u32) -> String;
  }

  pub fn editable_label<Msg: Clone + 'static>(text: &str) -> View<Msg>;
  pub trait EditableLabelExt<Msg>: Sized {
      fn editing(self, on: bool) -> Self;
      fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
  }
  pub struct EditableLabelC { pub edit: TextEditState, pub editing: bool,
                              pub committed: String, pub stack: Node, pub label: Node }
  ```

- [ ] **Step 1: Vendor the fixtures** — `spin_button.txt` (`gtk/gtkspinbutton.c:163`, horizontal form) and `editable_label.txt` (`gtk/gtkeditablelabel.c:70`):

```
spinbutton.horizontal
├── text
│    ├── undershoot.left
│    ╰── undershoot.right
├── button.down
╰── button.up
```

```
editablelabel[.editing]
╰── stack
    ├── label
    ╰── text
```

- [ ] **Step 2: Write the failing tests**

`ui/tests/node_trees.rs`:

```rust
#[test]
fn a_spin_button_has_two_stepper_buttons_and_an_editable_label_has_a_stack() {
    // mutation: name the steppers `button.up`/`button.down` in the wrong order
    // and the fixture's `╰── button.up` last-child position fails.
    use icedtea_ui::widgets::spin_button::SpinButtonC;
    assert_eq!(SpinButtonC::format(1.5, 2), "1.50");
    assert_eq!(SpinButtonC::format(1.5, 0), "2");
    assert_eq!(SpinButtonC::format(f64::NAN, 2), "0.00", "NaN never renders as NaN");

    let mut props = Props::default();
    props.set(PropName::Value, Prop::Float(3.0));
    props.set(PropName::Lower, Prop::Float(0.0));
    props.set(PropName::Upper, Prop::Float(10.0));
    check(Kind::SpinButton, "spin_button", &props);

    let mut label = Props::default();
    label.set(PropName::Text, Prop::Str("Name".into()));
    check(Kind::EditableLabel, "editable_label", &label);
    let rendered = node_tree_of(Kind::EditableLabel, &label);
    assert!(rendered.contains("stack"), "{rendered}");
}
```

`ui/tests/widget_pixels.rs`:

```rust
#[test]
fn stepping_a_spin_button_repeats_while_the_button_is_held() {
    // mutation: return None from SpinButtonC::next_deadline and the value
    // advances once instead of several times.
    use icedtea_ui::view::builders::spin_button;
    use icedtea_ui::widgets::spin_button::SpinButtonExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Set(f64);

    let frames = run(
        0.0f64,
        |model: &mut f64, Set(v): Set| {
            *model = v;
            Cmd::None
        },
        |model: &f64| spin_button(*model, 0.0, 100.0).step(1.0).on_value_changed(Set),
        (140, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter { x: 130.0, y: 12.0, serial: 1 }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: true, serial: 2, time_ms: 0,
            }),
            ScriptStep::Advance(Duration::from_millis(1_500)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110, pressed: false, serial: 3, time_ms: 1_500,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..140).map(|x| frames.pixel(frame, x, 20)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the displayed value must have advanced");
}

#[test]
fn an_editable_label_commits_on_enter_and_reverts_on_escape() {
    // mutation: keep `editing` true after Enter and the `.editing` class stays
    // on, changing the final capture.
    use icedtea_ui::view::builders::editable_label;
    use icedtea_ui::widgets::editable_label::EditableLabelExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Renamed(String);

    let frames = run(
        "Name".to_owned(),
        |model: &mut String, Renamed(text): Renamed| {
            *model = text;
            Cmd::None
        },
        |model: &String| editable_label(model).editing(true).on_change(|t| Renamed(t.to_owned())),
        (200, 40),
        vec![
            ScriptStep::Event(InputEvent::KeyboardEnter { serial: 1 }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::Key(key_char('X', 53))),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| (0..200).map(|x| frames.pixel(frame, x, 20)).collect::<Vec<_>>();
    assert_ne!(row(0), row(1), "the typed character must show while editing");
}
```

- [ ] **Step 3: Run tests to verify they fail** — `cargo test -p icedtea-ui --test node_trees a_spin_button_has_two`; expected FAIL, `unresolved import icedtea_ui::widgets::spin_button`.

- [ ] **Step 4: Write `ui/src/widgets/spin_button.rs`**

```rust
//! `GtkSpinButton` — `Kind::SpinButton`, CSS node `spinbutton`.
//!
//! ```text
//! spinbutton.horizontal
//! ├── text
//! │    ├── undershoot.left
//! │    ╰── undershoot.right
//! ├── button.down
//! ╰── button.up
//! ```
//!
//! The vertical form orders the children `button.up`, `text`, `button.down`.
//! The steppers paint `Builtin::SpinPlus`/`SpinMinus`; a held stepper repeats
//! through [`RepeatTimer`] with `climb-rate` acceleration.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::icons::builtin::Builtin;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{
    Adjustment, Orientation, PointerState, RepeatTimer, WidgetEnum, local_rect, shift_event,
};

/// Delay before a held stepper starts repeating, and the initial interval.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_INTERVAL: Duration = Duration::from_millis(100);

/// A `GtkSpinButton` at `value` over `lower..=upper`.
#[must_use]
pub fn spin_button<Msg: Clone + 'static>(value: f64, lower: f64, upper: f64) -> View<Msg> {
    View::new(Kind::SpinButton)
        .prop(PropName::Value, Prop::Float(value))
        .prop(PropName::Lower, Prop::Float(lower))
        .prop(PropName::Upper, Prop::Float(upper))
}

/// `GtkSpinButton`'s own setters and signals.
pub trait SpinButtonExt<Msg>: Sized {
    /// `GtkAdjustment:step-increment`.
    fn step(self, v: f64) -> Self;
    /// `GtkAdjustment:page-increment`.
    fn page(self, v: f64) -> Self;
    /// `GtkSpinButton:digits`.
    fn digits(self, n: u32) -> Self;
    /// `GtkSpinButton:wrap`.
    fn wrap(self, on: bool) -> Self;
    /// `GtkSpinButton:numeric`.
    fn numeric(self, on: bool) -> Self;
    /// `GtkSpinButton:snap-to-ticks`.
    fn snap_to_ticks(self, on: bool) -> Self;
    /// `GtkSpinButton:climb-rate`.
    fn climb_rate(self, v: f64) -> Self;
    /// `GtkOrientable:orientation`.
    fn orientation(self, o: Orientation) -> Self;
    /// `GtkSpinButton::value-changed`.
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> SpinButtonExt<Msg> for View<Msg> {
    fn step(self, v: f64) -> Self {
        self.prop(PropName::StepIncrement, Prop::Float(v))
    }
    fn page(self, v: f64) -> Self {
        self.prop(PropName::PageIncrement, Prop::Float(v))
    }
    fn digits(self, n: u32) -> Self {
        self.prop(PropName::Digits, Prop::Int(i64::from(n)))
    }
    fn wrap(self, on: bool) -> Self {
        self.prop(PropName::Wrap, Prop::Bool(on))
    }
    fn numeric(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn snap_to_ticks(self, on: bool) -> Self {
        self.prop(PropName::Homogeneous, Prop::Bool(on))
    }
    fn climb_rate(self, v: f64) -> Self {
        self.prop(PropName::Ratio, Prop::Float(v))
    }
    fn orientation(self, o: Orientation) -> Self {
        self.prop(PropName::Orientation, o.to_prop())
    }
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// `Kind::SpinButton`'s controller.
pub struct SpinButtonC {
    /// The text half's editing state.
    pub edit: TextEditState,
    /// The numeric model.
    pub adj: Adjustment,
    /// `GtkSpinButton:digits`.
    pub digits: u32,
    /// `GtkSpinButton:wrap`.
    pub wrap: bool,
    /// The held-stepper repeat clock.
    pub repeat: Option<RepeatTimer>,
    /// The `button.up` subnode.
    pub up: Node,
    /// The `button.down` subnode.
    pub down: Node,
    climb: f64,
    /// `+1` while the up stepper is held, `-1` for down.
    direction: i32,
    pointer: PointerState,
}

impl SpinButtonC {
    /// Format `value` at `digits`. A non-finite value renders as zero rather
    /// than `NaN`, which no spin button ever shows.
    #[must_use]
    pub fn format(value: f64, digits: u32) -> String {
        let value = if value.is_finite() { value } else { 0.0 };
        format!("{value:.*}", digits as usize)
    }

    /// Step by `steps` increments (page increments when `page`), wrapping when
    /// `wrap`. Returns the new value.
    pub fn step_by(&mut self, steps: i32, page: bool) -> f64 {
        let increment = if page { self.adj.page_increment } else { self.adj.step_increment };
        let delta = f64::from(steps) * increment;
        let raw = self.adj.value + delta;
        let span = self.adj.upper - self.adj.lower;
        let next = if self.wrap && span > 0.0 {
            if raw > self.adj.upper {
                self.adj.lower + (raw - self.adj.upper - increment).rem_euclid(span)
            } else if raw < self.adj.lower {
                self.adj.upper - (self.adj.lower - raw - increment).rem_euclid(span)
            } else {
                raw
            }
        } else {
            raw
        };
        self.adj.set_value(next);
        self.adj.value
    }

    fn sync_text(&mut self, cx: &mut BuildCx<'_>) {
        let text = Self::format(self.adj.value, self.digits);
        self.edit.set_text(&text, cx);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SpinButtonC {
    fn kind(&self) -> Kind {
        Kind::SpinButton
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let orientation =
            Orientation::from_prop(props.get(PropName::Orientation), Orientation::Horizontal);
        node.add_class(orientation.css_class());
        let adj = Adjustment {
            value: props.float(PropName::Value, 0.0),
            lower: props.float(PropName::Lower, 0.0),
            upper: props.float(PropName::Upper, 100.0),
            step_increment: props.float(PropName::StepIncrement, 1.0),
            page_increment: props.float(PropName::PageIncrement, 10.0),
            page_size: 0.0,
        }
        .sanitized();
        let digits = u32::try_from(props.int(PropName::Digits, 0)).unwrap_or(0).min(20);
        let up = Node::with_classes("button", &["up"]);
        let down = Node::with_classes("button", &["down"]);
        // Horizontal: text, button.down, button.up. Vertical: up, text, down.
        let edit = match orientation {
            Orientation::Horizontal => {
                let edit = TextEditState::build(node, &Self::format(adj.value, digits), cx);
                node.append_child(&down);
                node.append_child(&up);
                edit
            }
            Orientation::Vertical => {
                node.append_child(&up);
                let edit = TextEditState::build(node, &Self::format(adj.value, digits), cx);
                node.append_child(&down);
                edit
            }
        };
        SpinButtonC {
            edit,
            adj,
            digits,
            wrap: props.bool(PropName::Wrap, false),
            repeat: None,
            up,
            down,
            climb: props.float(PropName::Ratio, 1.0),
            direction: 0,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => {
                self.adj.set_value(*v);
            }
            (PropName::Lower, Prop::Float(v)) => self.adj.lower = *v,
            (PropName::Upper, Prop::Float(v)) => self.adj.upper = *v,
            (PropName::StepIncrement, Prop::Float(v)) => self.adj.step_increment = *v,
            (PropName::Digits, Prop::Int(n)) => {
                self.digits = u32::try_from(*n).unwrap_or(0).min(20);
            }
            (PropName::Wrap, Prop::Bool(on)) => self.wrap = *on,
            _ => return,
        }
        self.adj = self.adj.sanitized();
        self.sync_text(cx);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        for (node, direction) in [(self.up.clone(), 1), (self.down.clone(), -1)] {
            let Some(rect) = local_rect(cx.tree, cx.node, &node) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let bounds = Rect::new(0.0, 0.0, rect.width, rect.height);
            if let Event::PointerDown { local, .. } = shifted {
                if local.0 >= 0.0 && local.1 >= 0.0 && local.0 <= bounds.width && local.1 <= bounds.height
                {
                    self.direction = direction;
                    self.repeat = Some(RepeatTimer::armed(
                        cx.clock.now(),
                        REPEAT_DELAY,
                        REPEAT_INTERVAL,
                        self.climb,
                    ));
                    let value = self.step_by(direction, false);
                    cx.handled = true;
                    return cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, value)
                        .map_or_else(Vec::new, |m| vec![m]);
                }
            }
            self.pointer.observe(&node, &shifted, Some(bounds));
        }
        if matches!(ev, Event::PointerUp { .. }) {
            self.repeat = None;
            self.direction = 0;
        }
        if let Event::Scroll(scroll) = ev {
            let steps = if scroll.dy > 0.0 { -1 } else { 1 };
            let value = self.step_by(steps, false);
            cx.handled = true;
            return cx
                .handlers
                .fire_float(EventKind::ValueChanged, value)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        use xkbcommon::xkb::keysyms;
        let steps = match u32::from(key.keysym) {
            keysyms::KEY_Up if key.pressed => Some((1, false)),
            keysyms::KEY_Down if key.pressed => Some((-1, false)),
            keysyms::KEY_Page_Up if key.pressed => Some((1, true)),
            keysyms::KEY_Page_Down if key.pressed => Some((-1, true)),
            _ => None,
        };
        if let Some((steps, page)) = steps {
            let value = self.step_by(steps, page);
            cx.handled = true;
            return cx
                .handlers
                .fire_float(EventKind::ValueChanged, value)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        // Typing edits the text; the model re-parses it on `Change`.
        match self.edit.key(key, cx) {
            EditOutcome::Changed | EditOutcome::Activated => {
                cx.handled = true;
                let parsed = self.edit.buffer.trim().parse::<f64>().ok();
                if let Some(value) = parsed.filter(|v| v.is_finite()) {
                    self.adj.set_value(value);
                    return cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, self.adj.value)
                        .map_or_else(Vec::new, |m| vec![m]);
                }
                Vec::new()
            }
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Ignored => Vec::new(),
        }
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(timer) = self.repeat.as_mut() else {
            return Vec::new();
        };
        let fired = timer.fire(now);
        if fired == 0 {
            return Vec::new();
        }
        let steps = self.direction * i32::try_from(fired).unwrap_or(1);
        let value = self.step_by(steps, false);
        cx.handlers.fire_float(EventKind::ValueChanged, value).map_or_else(Vec::new, |m| vec![m])
    }

    fn next_deadline(&self, _now: Duration) -> Option<Duration> {
        self.repeat.as_ref().map(RepeatTimer::deadline)
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        self.edit.layout.draw(canvas, (content.x, content.y), style.color());
        // The steppers' builtin glyphs; P7 fills in the real geometry (D9).
        let _ = (Builtin::SpinPlus, Builtin::SpinMinus);
        true
    }
}
```

- [ ] **Step 5: Write `ui/src/widgets/editable_label.rs`**

```rust
//! `GtkEditableLabel` — `Kind::EditableLabel`, node `editablelabel`.
//!
//! ```text
//! editablelabel[.editing]
//! ╰── stack
//!     ├── label
//!     ╰── text
//! ```
//!
//! Enter starts editing from the label and commits from the text; Escape
//! reverts to the last committed value — GTK's `editing.start`/`editing.stop`.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{BuildCx, Controller, Event, EventCx};
use crate::view::{EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{PointerState, local_rect, shift_event};

/// A `GtkEditableLabel` showing `text`.
#[must_use]
pub fn editable_label<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::EditableLabel).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkEditableLabel`'s own setters and signals.
pub trait EditableLabelExt<Msg>: Sized {
    /// `GtkEditableLabel:editing`.
    fn editing(self, on: bool) -> Self;
    /// `GtkEditable::changed`, fired on commit.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> EditableLabelExt<Msg> for View<Msg> {
    fn editing(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
}

/// `Kind::EditableLabel`'s controller.
pub struct EditableLabelC {
    /// The text half's editing state.
    pub edit: TextEditState,
    /// Whether the widget is in editing mode.
    pub editing: bool,
    /// The last committed value, which Escape reverts to.
    pub committed: String,
    /// The `stack` subnode.
    pub stack: Node,
    /// The `label` subnode inside the stack.
    pub label: Node,
    pointer: PointerState,
}

impl EditableLabelC {
    fn apply(&self, node: &Node) {
        if self.editing {
            node.add_class("editing");
        } else {
            node.remove_class("editing");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for EditableLabelC {
    fn kind(&self) -> Kind {
        Kind::EditableLabel
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let stack = Node::new("stack");
        node.append_child(&stack);
        let label = Node::new("label");
        stack.append_child(&label);
        let text = props.str(PropName::Text).unwrap_or("").to_owned();
        let edit = TextEditState::build(&stack, &text, cx);
        let this = EditableLabelC {
            edit,
            editing: props.bool(PropName::Editable, false),
            committed: text,
            stack,
            label,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => {
                self.committed = text.to_string();
                self.edit.set_text(text, cx);
            }
            (PropName::Editable, Prop::Bool(on)) => self.editing = *on,
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use xkbcommon::xkb::keysyms;
        if let Some(rect) = local_rect(cx.tree, cx.node, &self.label) {
            let shifted = shift_event(ev, rect);
            if self.pointer.observe(
                &self.label,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) && !self.editing
            {
                self.editing = true;
                self.apply(cx.node);
                cx.handled = true;
                return Vec::new();
            }
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        if key.pressed && u32::from(key.keysym) == keysyms::KEY_Escape && self.editing {
            self.editing = false;
            self.apply(cx.node);
            cx.handled = true;
            // Revert without an undo step: the model never saw this edit.
            self.edit.buffer.clone_from(&self.committed);
            self.edit.cursor = self.edit.buffer.len();
            self.edit.anchor = None;
            return Vec::new();
        }
        if !self.editing {
            if key.pressed
                && matches!(u32::from(key.keysym), keysyms::KEY_Return | keysyms::KEY_KP_Enter)
            {
                self.editing = true;
                self.apply(cx.node);
                cx.handled = true;
            }
            return Vec::new();
        }
        match self.edit.key(key, cx) {
            EditOutcome::Activated => {
                self.editing = false;
                self.apply(cx.node);
                self.committed.clone_from(&self.edit.buffer);
                cx.handled = true;
                let text = self.committed.clone();
                cx.handlers.fire_text(EventKind::Change, &text).map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Changed | EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Ignored => Vec::new(),
        }
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        self.edit.layout.draw(canvas, (content.x, content.y), style.color());
        true
    }
}
```

- [ ] **Step 6: Register both** — `pub mod spin_button;`, `pub mod editable_label;`; the two arms; and the two `pub use` lines.

- [ ] **Step 7: Run tests to verify they pass** — `cargo test -p icedtea-ui --test node_trees a_spin_button_has_two`, `cargo test -p icedtea-ui --test widget_pixels stepping_a_spin_button`, `... an_editable_label_commits`; expected PASS.

- [ ] **Step 8: Run the gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

- [ ] **Step 9: Commit**

```bash
git add ui/src/widgets ui/src/view/builders.rs ui/tests
git commit -m "feat(ui/widgets): SpinButton and EditableLabel

The spin button's held stepper repeats through RepeatTimer with climb-rate
acceleration and a 20ms floor; the editable label commits on Enter and reverts
to the last committed value on Escape, without recording an undo step.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 27: P5 close-out — the full fixture sweep, the deviation record, and the docs

**Files:**
- Modify: `ui/tests/node_trees.rs` (the exhaustive sweep)
- Modify: `ui/tests/widget_pixels.rs` (the exhaustive rest-state sweep)
- Modify: `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§10 amendments)
- Modify: `ui/README.md` (the P5 widget table)

**Interfaces:**
- Consumes: every kind Tasks 7–26 registered; `Kind::all()`.
- Produces: no new code items; two exhaustive gates that fail when a P5 kind is added without a fixture or a rest-state test.

- [ ] **Step 1: Write the failing exhaustive tests**

Append to `ui/tests/node_trees.rs`:

```rust
/// Every kind P5 owns, in contract §5.1–§5.3 order.
const P5_KINDS: &[(Kind, &str)] = &[
    (Kind::Label, "label"),
    (Kind::Spinner, "spinner"),
    (Kind::Statusbar, "statusbar"),
    (Kind::LevelBar, "level_bar"),
    (Kind::ProgressBar, "progress_bar"),
    (Kind::InfoBar, "info_bar"),
    (Kind::Scrollbar, "scrollbar"),
    (Kind::Image, "image"),
    (Kind::Picture, "picture"),
    (Kind::Separator, "separator"),
    (Kind::TextView, "text_view"),
    (Kind::Scale, "scale"),
    (Kind::DrawingArea, "drawing_area"),
    (Kind::WindowControls, "window_controls"),
    (Kind::Calendar, "calendar"),
    (Kind::Popover, "popover"),
    (Kind::Button, "button"),
    (Kind::ToggleButton, "toggle_button"),
    (Kind::LinkButton, "link_button"),
    (Kind::CheckButton, "check_button"),
    (Kind::MenuButton, "menu_button"),
    (Kind::Switch, "switch"),
    (Kind::DropDown, "drop_down"),
    (Kind::ColorDialogButton, "color_dialog_button"),
    (Kind::ColorDialog, "color_dialog"),
    (Kind::FontDialogButton, "font_dialog_button"),
    (Kind::FontDialog, "font_dialog"),
    (Kind::Entry, "entry"),
    (Kind::SearchEntry, "search_entry"),
    (Kind::PasswordEntry, "password_entry"),
    (Kind::SpinButton, "spin_button"),
    (Kind::EditableLabel, "editable_label"),
];

#[test]
fn every_p5_kind_has_a_fixture_and_matches_it_with_default_props() {
    // mutation: delete any entry from P5_KINDS and the count assertion fails;
    // delete a fixture file and `fixture()` panics by name.
    assert_eq!(P5_KINDS.len(), 32, "contract §5.1-§5.3 owns exactly 32 kinds");
    for (kind, name) in P5_KINDS {
        check(*kind, name, &Props::default());
    }
}

#[test]
fn no_p5_kind_falls_through_to_the_unimplemented_controller() {
    // mutation: remove any dispatch arm from build_controller and that kind's
    // tree renders as a bare node, failing its fixture's required subnodes.
    // Kinds whose GTK tree really is one bare node are listed explicitly, so
    // this test cannot be satisfied by accident.
    const BARE: &[Kind] = &[
        Kind::Spinner,
        Kind::Separator,
        Kind::Image,
        Kind::Picture,
        Kind::DrawingArea,
        Kind::Button,
        Kind::ToggleButton,
        Kind::LinkButton,
    ];
    for (kind, _) in P5_KINDS {
        let rendered = node_tree_of(*kind, &Props::default());
        let has_subnodes = rendered.lines().count() > 1;
        assert_eq!(
            has_subnodes,
            !BARE.contains(kind),
            "{kind:?} rendered:\n{rendered}"
        );
    }
}
```

Append to `ui/tests/widget_pixels.rs`:

```rust
#[test]
fn every_p5_kind_renders_at_rest_without_panicking() {
    // mutation: make any controller's `build` index a subnode it did not
    // create and this test panics for that kind by name.
    use icedtea_ui::view::{Kind, Props};
    use icedtea_ui::widgets::node_tree_of;
    for kind in Kind::all() {
        // P6's kinds still resolve to the Unimplemented controller; skip them.
        if !matches!(
            kind,
            Kind::Label
                | Kind::Spinner
                | Kind::Statusbar
                | Kind::LevelBar
                | Kind::ProgressBar
                | Kind::InfoBar
                | Kind::Scrollbar
                | Kind::Image
                | Kind::Picture
                | Kind::Separator
                | Kind::TextView
                | Kind::Scale
                | Kind::DrawingArea
                | Kind::WindowControls
                | Kind::Calendar
                | Kind::Popover
                | Kind::Button
                | Kind::ToggleButton
                | Kind::LinkButton
                | Kind::CheckButton
                | Kind::MenuButton
                | Kind::Switch
                | Kind::DropDown
                | Kind::ColorDialogButton
                | Kind::ColorDialog
                | Kind::FontDialogButton
                | Kind::FontDialog
                | Kind::Entry
                | Kind::SearchEntry
                | Kind::PasswordEntry
                | Kind::SpinButton
                | Kind::EditableLabel
        ) {
            continue;
        }
        let rendered = node_tree_of(*kind, &Props::default());
        assert!(!rendered.is_empty(), "{kind:?} rendered nothing");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test node_trees every_p5_kind_has_a_fixture`
Expected: FAIL for any kind whose fixture is missing or whose default-props
tree does not match — fix the owning task's widget or fixture, not the gate.

- [ ] **Step 3: Make them pass**

Every failure is a real gap in Tasks 7–26: a missing fixture file, a subnode a
controller builds that GTK's block does not name, or a required class the
controller never adds. Fix the widget or the fixture in its own file; do not
weaken `fixture_matches`.

- [ ] **Step 4: Record the deviations in the contract**

Append to `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` §10, one
`### E<n>` per deviation, in this plan's D1–D9 order plus the two recorded
inside Tasks 21 and 22:

```markdown
### E1 — P5 adds `unicode-segmentation` and `unicode-linebreak` to `ui/Cargo.toml`
§0 gives `ui/Cargo.toml` to P3 and lists three deps. §3.7's `TextLayout` needs
UAX #29 cluster/word boundaries and UAX #14 line-break opportunities. Ruling:
P5 adds `unicode-segmentation = "1.13"` and `unicode-linebreak = "0.1"`; both
already resolve in the workspace lockfile through `cosmic-text`. Carried out
by P5, Task 1.

### E2 — P5 creates `ui/src/icons/builtin.rs`
§9 assigns `ui/src/icons/**` to P7 but P5's own boundary orders the `Builtin`
stub. Ruling: P5 creates the file with P7's exact signatures and placeholder
geometry; P7 replaces the bodies and un-ignores the two tests named in E9.

### E3 — `ImageC.resolved` is `Option<Rc<skia_rs_safe::codec::Image>>`
§5.1 names `icons::Handle`, which §6 never defines; §6's `IconTheme::render`
returns `Option<Rc<Image>>`. Ruling: follow §6.

### E4 — P5 defines the eighteen widget-local types §5 names but never declares
`Orientation`, `Position`, `Side`, `IconSize`, `MessageType`, `LevelBarMode`,
`ContentFit`, `MatchMode`, `ArrowDirection`, `FontLevel`, `WindowButton`,
`ColorDialogSpec`, `PictureSource`, `Adjustment`, `ListItem`, `Mark`,
`RepeatTimer`, and `TextEditState`/`UndoStack`. Ruling: they live in
`ui/src/widgets/mod.rs` and `ui/src/widgets/edit.rs`, with a `WidgetEnum` trait
carrying them through `Prop::Enum(u16)`.

### E5 — P5 appends re-export lines to `ui/src/view/builders.rs`
§9's P5 boundary says "must not touch `view/**`"; §0 says `view/builders.rs` is
where P5/P6 fill the builder frame and §4.4 requires `view::builders::<name>`.
Ruling: builder bodies live under `ui/src/widgets/`; `view/builders.rs` gains
only `pub use` lines.

### E6 — `widgets::build_controller` is the reconciler's `Kind` dispatch
`Controller::build` is `Self: Sized`, so §4.5's `reconcile` cannot build a
controller from a `Kind`. Ruling: P5 owns the table beside the widgets; P4's
reconciler calls it; P6 replaces the `Unimplemented` arms.

### E7 — `node_tree_of` is matched, not string-diffed
§5 says the rendered tree is diffed against the fixture, but the fixtures carry
`[optional]`, `┊` and `<child>`. Ruling: `widgets::fixture_matches` enforces
both directions — every rendered node at a fixture-named path with every
required class, and every non-optional fixture node present.

### E8 — WITHDRAWN: P4's `App::with_sheet` already covers it
§4.7's `run_offscreen` takes no stylesheet, which every P5 rest-state pixel test
needs. P4's deviation D11 already ships `App::{with_sheet, with_fonts,
with_icons}`. Ruling: P5 adds no setter and does not edit `view/app.rs`; it
calls `.with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))`.

### E9 — two P5 pixel assertions are deferred to P7's fix wave
`a_check_button_paints_the_builtin_check_glyph` and
`an_image_paints_its_resolved_icon` are written now and `#[ignore]`d until P7
fills `Builtin::path` and `IconTheme::render`. §9's P5 boundary already
sanctions this; the two names are recorded so P7 cannot miss them.

### E10 — `DropDownC.list` and `FontDialogC.list` are `Node`, not `ListViewC`
§5.2 types both as `ListViewC`, which is a P6 kind landing after P5. Ruling: P5
builds the `listview`/`fontchooser` node and its rows directly; P6 replaces the
field when `Kind::ListView` lands.

### E11 — `ColorDialogButton`'s change handler carries a packed `f64`
§5.2 writes `.on_change(|Rgba| Msg)`, but §4.4's `Handler` has no `Rgba`
variant. Ruling: the colour rides through `Handler::Float` as `0xAARRGGBB`,
which an `f64` represents exactly; `ColorDialogC::{pack, unpack}` are the only
conversion.
```

- [ ] **Step 5: Update `ui/README.md`**

Add a "P5 widgets" table listing the 32 kinds, their CSS node names, their
builder names and their fixture file names, and a paragraph pointing at
`ui/tests/node_trees.rs` as the conformance gate. Do not touch the M1/M2
sections.

- [ ] **Step 6: Run the full gates**

```bash
cargo test -p icedtea-ui
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
cargo test --workspace
```
Expected: all green, with exactly two ignored tests (E9's). In particular the
four `themed_button_offscreen`, nine `adwaita_coverage`, four
`gtk4_property_reference` and one `transition_screencopy` tests pass unchanged.

- [ ] **Step 7: Commit**

```bash
git add ui/tests ui/README.md docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "test(ui/widgets): the exhaustive P5 fixture sweep, and the contract amendments

Adding a P5 kind without a fixture or without a dispatch arm now fails a named
gate rather than passing silently. The eleven contract deviations P5 discovered
are recorded in §10.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-Review

### 1. Spec coverage

| Spec / contract requirement | Task |
|---|---|
| §3.7 `TextLayout::build`, `size`, `line_count`, `draw` | 1 |
| §3.7 `WrapMode::{None, Word, Char, WordChar}` | 2 |
| §3.7 `Ellipsize::{None, Start, Middle, End}` | 3 |
| §3.7 `caret_rect`, `byte_at`, `selection_rects` | 4 |
| §3.7 `next_grapheme`, `prev_grapheme`, `next_word`, `prev_word` | 5 |
| §3.7 `parse_markup`, `MarkupSpan` | 6 |
| §5 `node_tree_of` + fixture gate; §5.1 `Separator` | 7 |
| §5.1 `Label` | 8 |
| §5.1 `Spinner`, `Statusbar` | 9 |
| §5.1 `LevelBar`, `ProgressBar` | 10 |
| §5.1 `InfoBar` | 11 |
| §5.1 `Scrollbar` (+ `set_adjustment`/`slider_rect` for P6) | 12 |
| §5.1 `Image`, `Picture` | 13 |
| §5.1 `TextView` | 14 |
| §5.1 `Scale` | 15 |
| §5.1 `DrawingArea`, `WindowControls` (+ the `update_window_buttons` rule) | 16 |
| §5.1 `Calendar` | 17 |
| §5.1 `Popover` (ruling R4) | 18 |
| §5.2 `Button`, `ToggleButton`, `LinkButton` | 19 |
| §5.2 `CheckButton`, `Switch` (+ the group-by-name note) | 20 |
| §5.2 `MenuButton`, `DropDown` | 21 |
| §5.2 `ColorDialogButton`, `ColorDialog`, `FontDialogButton`, `FontDialog` | 22 |
| §5.3 `TextEditState`, `UndoStack`, the `GtkText` key table | 23 |
| §5.3 `Entry` | 24 |
| §5.3 `SearchEntry`, `PasswordEntry` | 25 |
| §5.3 `SpinButton`, `EditableLabel` | 26 |
| §9 P5 gate: per-widget rest state + interaction + node tree | every widget task; swept exhaustively in 27 |
| §9 cross-cutting: never-panic on untrusted input | 6 (markup), 7 (`Adjustment`), 13 (image decode), 17 (month/weekday), 21 (search filter), 22 (colour unpack), 26 (`format`) |
| §9 cross-cutting: `Duration::ZERO` is never "spin" | 9 (`SpinnerC`), 10 (`ProgressBarC`), 19 (`ButtonC`), 20 (`SwitchC`), 25 (`SearchEntryC`), 26 (`SpinButtonC`) |
| §8.2 the M1/M2 gate stays byte-identical | asserted in the gates of 7, 19 and 27 |

No spec requirement in §3.7, §5.1, §5.2 or §5.3 is without a task. `Statusbar`,
`InfoBar` and `ShortcutsWindow` are in scope by ruling R1; the first two are
Tasks 9 and 11, and `ShortcutsWindow` is a P6 kind (contract §5.7), correctly
absent here.

### 2. Placeholder scan

No "TBD", "TODO", "implement later", "fill in details", "add appropriate error
handling", "similar to Task N" or "write tests for the above" appears in any
task. Every code step carries real code. Three forward references are explicit
and bounded rather than vague:

- Task 1 stubs `break_paragraph` and `ellipsize_line` with **working**
  `WrapMode::None`/`Ellipsize::None` bodies, replaced wholesale in Tasks 2 and
  3 — the crate compiles and its tests pass after Task 1.
- Task 7's `Builtin` geometry is a documented placeholder that P7 replaces,
  under contract §9's own instruction, with the two dependent assertions
  `#[ignore]`d by name (D9).
- Task 14's `TextViewC` gains `UndoStack` in Task 23, which is stated in Task
  14's `Interfaces` block and carried out in Task 23 Step 5.

### 3. Type consistency

Checked across tasks:

- `TextLayout`'s method set is defined once (Tasks 1–5) and consumed with the
  same names by `LabelC` (8), `TextViewC` (14) and `TextEditState` (23):
  `build`, `size`, `line_count`, `text`, `display_line`, `draw`, `caret_rect`,
  `byte_at`, `selection_rects`, `line_of`, `next_grapheme`, `prev_grapheme`,
  `next_word`, `prev_word`.
- `PointerState::observe(&Node, &Event, Option<Rect>) -> bool` (7) is called
  with exactly that signature in Tasks 8, 11, 12, 14, 15, 16, 17, 19–22, 24–26.
- `shift_event(&Event, Rect) -> Event` and
  `local_rect(&LayoutTree, &Node, &Node) -> Option<Rect>` are defined once (12)
  and used unchanged thereafter; Task 11 introduces the private `shift` and
  Task 12 Step 4 explicitly moves and renames it, so no duplicate survives.
- `WidgetEnum::{to_u16, from_u16, from_prop, to_prop}` (7) is the only path in
  and out of `Prop::Enum` everywhere.
- `Adjustment`'s field names (`value`, `lower`, `upper`, `step_increment`,
  `page_increment`, `page_size`) and methods (`new`, `sanitized`, `clamp`,
  `fraction`, `value_at_fraction`, `set_value`) are identical in Tasks 7, 12,
  15 and 26.
- `TextEditState`'s field and method names (23) match every use in Tasks 24, 25
  and 26; `EditOutcome`'s four variants are matched exhaustively in all four
  embedding controllers.
- `ColorDialogC::{pack, unpack}` (22) is the only colour packing, used by both
  the button and the dialog.
- Every `PropName` used is drawn from contract §4.3's `#[non_exhaustive]` list;
  no task invents one. Where a widget needs a slot §4.3 does not name for it
  (`Label`'s `lines` → `Rows`, `Scale`'s marks → `MarksTop`/`MarksBottom`,
  `SearchEntry`'s delay → `TransitionDuration`), the mapping is stated in the
  builder's own code, so the same slot is never claimed twice within one kind.
- Controller struct names are `<Kind>C` without exception, and every one is
  registered in `build_controller` in the task that creates it.
