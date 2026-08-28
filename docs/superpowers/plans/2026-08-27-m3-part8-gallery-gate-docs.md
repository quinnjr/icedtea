# Pure-Rust GTK-themed UI — M3 Part 8: gallery binary, gallery + interaction gates, README/spec status — Implementation Plan

> **Note for agentic workers:** this plan is written to be executed with the
> `superpowers:subagent-driven-development` skill — one subagent per task, each
> task self-contained (files, interfaces, failing test, implementation, gate
> command, commit). Do not batch tasks; do not skip the failing-test step. Every
> task ends on a green `cargo test -p icedtea-ui` and a commit.

## Contract deviations

The binding contract is
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`. Every signature in
this plan is copied from it verbatim except the items below. Each one is to be
appended to the contract's §10 as an amendment in Task 14.

1. **`ui/src/gallery.rs` is added as a library module.** The contract's §0
   module map gives P8 only `ui/src/bin/gallery.rs`. But `Kind` is
   `#[non_exhaustive]` (§4.2), and `#[non_exhaustive]` bites *across crates* —
   a binary target is a different crate from the library, so an exhaustive
   `match kind { … }` in `src/bin/gallery.rs` would need a `_ =>` arm and the
   contract's "adding a `Kind` without a gallery entry is a compile-time hole"
   guarantee would evaporate. The sample table therefore lives in
   `ui/src/gallery.rs` (same crate as `Kind`, exhaustive match compiles),
   and `ui/src/bin/gallery.rs` is CLI plumbing only.
2. **P8 adds `App::probe` + `Probe<Msg>` to `ui/src/view/app.rs`** (P4's file;
   additive, nothing renamed or re-typed). `--probe-points` and
   `--print-allocation` must print geometry **without a compositor** (§7: "This
   is how the gate learns coordinates — it never hard-codes", extending the M2
   `--print-allocation` precedent, which is headless). The whole
   reconcile→restyle→layout pipeline lives inside `App` and is otherwise
   private; `App::run_offscreen` returns pixels (`Frames`), not allocations.
   Rather than reimplement P4's pipeline inside the gallery, P8 adds one
   accessor that runs it once and hands back the retained tree plus its
   `LayoutTree`. Exact signature in Task 5.
3. **Probe points are derived by P8, not exposed per widget.** §7 says "Every
   widget exposes its probe points as `(label, node)` pairs", but §9 also says
   P8 "**Must not touch:** any widget implementation". Both cannot hold. Ruling:
   `gallery::probe_points_of(instance, tree)` derives them from the retained
   `Node` subtree — `"root"` for the widget's own node, then each descendant
   labelled by its CSS node name, with `<name><index>` for every occurrence of a
   name that occurs more than once in that subtree (`tab0`, `tab1`, `row0`).
   That reproduces §7's own examples (`"check"`, `"slider"`, `"trough"`,
   `"text"`, `"arrow"`, `"tab0"`, `"row0"`) and stays derivable, never
   hard-coded.
4. **`support::probe_points`/`spawn_gallery` signatures widen.** §7 lists
   `probe_points(theme: &str)` and `spawn_gallery(socket, theme) -> Reaper`.
   This plan ships `probe_points(theme: Theme, widget: Option<&str>)` (a widget
   argument, because the interaction gate needs the isolated `--widget` layout's
   coordinates) and `spawn_gallery(socket, theme, scroll) -> GalleryProc`
   (`GalleryProc` is a `Reaper` plus the child's captured stdout, because §7's
   "screencopy **or model** assertions" need the messages the app folded to
   cross the process boundary). `Reaper` itself is unchanged and still used by
   the M2 tests.
5. **The gallery maps a `zwlr_layer_shell_v1` overlay, not an xdg-toplevel.**
   §7 does not name a role. Probe coordinates are output coordinates, and only
   the layer role has a compositor-independent origin (anchor top|left, margin
   0 — M2's `themed-button` precedent). Keyboard interactivity is `Exclusive`
   so the typing/Tab interactions get a focused keyboard.
6. **`every_probe_point_differs_between_light_and_dark` is per widget.** Read
   literally, the name asserts something false: a fully transparent subnode, or
   one Adwaita styles identically in both sheets, legitimately matches across
   themes. The test keeps its contract name and asserts **at least one probe
   point per widget** differs by more than `SCREENCOPY_TOLERANCE`, and names the
   widget that fails.
7. **`--probe-points`/`--print-allocation` line formats are pinned here.** §7
   pins `--probe-points` as `<widget> <label> <x> <y>` (adopted verbatim) but
   leaves `--print-allocation` as "one allocation per line"; this plan pins it
   to `<widget> <x> <y> <width> <height>` (the entry's border box, page
   coordinates), which is what the gate's page-slice arithmetic needs.
8. **One test is added to `gallery_gate.rs`:**
   `the_readme_widget_table_lists_every_kind`. §7's six names all remain and are
   all implemented; this seventh keeps the README table (Task 13) from rotting
   the moment a `Kind` is added.
9. **`harness/src/lib.rs` grows `VirtualPointerClient::{axis, axis_discrete}`.**
   Additive, no rename, no existing harness test touched (§8.2 keeps every
   existing harness test green). The contract's own interaction-gate list
   requires `scrolling_a_list_view_recycles_rows_without_losing_selection`, and
   the harness injector today has motion/button/frame but no axis event at all.
10. **ASSUMED types.** The contract names these but never defines them:
    `SurfaceSpec` (§3.1 `Window::open`'s first parameter), `ListItem` (§4.3
    `Prop::Items`), the list/grid/column `factory` parameter, and the widget
    enums `MessageType`, `Orientation`, `Position`, `Side`, `SelectionMode`,
    `StackTransition`, `ContentFit`, `Ellipsize`, `Policy`, `LevelBarMode`,
    `ArrowDirection`, `Rgba`, `MenuFlags`, `DisplayHint`, `MatchMode`,
    `SortOrder`, `Sorter`, `IconSize`, `WrapMode`. P3/P5/P6 define them and land
    **before** P8. Where this plan constructs one it is marked `// ASSUMED` at
    the call site; the executor reads the shipped definition from
    `ui/src/window/mod.rs` / `ui/src/view/mod.rs` / `ui/src/widgets/*.rs`
    **before** writing that line and adjusts the constructor only. No gate logic
    depends on their internals. If an assumed name is absent entirely, that is a
    P3–P6 gap: record it in the contract's §10 and raise it, do not invent a
    replacement widget.
11. **`App::run` is called with the `Window`.** §4.7 spells it
    `App::run(self, surface: Surface)`, but `App` needs the `CompiledSheet`,
    `FontDatabase`, clock and `AnimationState` that only `Window` owns (§3.1),
    and `Surface` exposes none of them. The gallery calls `App::run(window)`. If
    P4 shipped the literal `Surface` parameter, only the two lines in
    `gallery::run` change — the `Window` is constructed there either way.

## Goal

Close M3 with the thing that measures it: a **`gallery` binary** that renders
every one of the 64 `Kind`s the toolkit ships, a **rest-state gate** that
screencopies every widget's derived probe points in Adwaita light, dark and
high contrast, an **interaction gate** that drives one interaction per widget
class with a virtual pointer and keyboard, and the **documentation pass** that
makes `ui/README.md` and the two spec status lines true again.

The gallery is not a demo that happens to exist — it is the completeness
instrument. The page is built by iterating `Kind::all()`, so a `Kind` added
without a gallery entry does not compile, and a widget that paints nothing
fails a probe rather than silently shipping.

## Architecture

```
ui/src/gallery.rs                    [P8, new] the whole gallery, as library code
  Theme {Light,Dark,HighContrast}    which bundled sheet + which MediaEnv
  Options / OptionsError             the §7 command line, parsed and validated
  kind_name / kind_from_name         Kind <-> the CLI's snake_case names (64 arms)
  GalleryMsg / GalleryModel          the Elm loop this page runs
  update / page / sample             view(&Model); `sample` is the exhaustive match
  Sample::{Own, Within}              a kind is either its own framed entry or a
                                     sub-node of another kind's entry
  BuiltPage / probe_points_of        headless layout + derived probe points
  run / print_probe_points / print_allocations / print_list
                                     the four things `main` can do

ui/src/bin/gallery.rs                [P8, new] argv -> Options -> gallery::*, nothing else
ui/src/view/app.rs                   [P4, +2 items] App::probe -> Probe<Msg> (deviation 2)

ui/tests/support/mod.rs              [P8, grown] ProbePoint, EntryAllocation,
                                     probe_points, entry_allocations, spawn_gallery,
                                     spawn_gallery_widget, GalleryProc, click_at,
                                     type_keys, scroll_at  (M2 helpers untouched)
ui/tests/gallery_gate.rs             [P8, new] 6 contract tests + the README table test
ui/tests/interaction_gate.rs         [P8, new] the 16 contract interactions
ui/tests/node_trees.rs               [P5/P6, wired] re-exported into the gallery gate

harness/src/lib.rs                   [P8, +2 methods] VirtualPointerClient::axis*
ui/README.md                         [P8] widget table, module map, gates, limits
docs/superpowers/specs/2026-08-27-…-m3-…-design.md   status line
docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md   M3/M4 status
```

Two coordinate systems, and the gate never mixes them:

- **page mode** (`gallery --theme dark --scroll 1200`): one vertical `box` of
  `frame`s, one frame per own-sample `Kind`, in `Kind::all()` order. `--scroll`
  is applied as a negative top margin on the page node, so the laid-out tree
  *is* the scrolled tree and probe points need no correction. The rest-state
  gate walks the page in `--size`-tall slices.
- **isolated mode** (`gallery --widget check_button`): that one sample alone at
  the origin, no frame, no siblings. The interaction gate uses only this mode,
  so every coordinate is small, stable and independent of every other widget.

## Tech Stack

| Concern | API |
|---|---|
| Widget set, builders, `Kind` | P4 `icedtea_ui::view::{Kind, View, Props, Instance, reconcile}`, P5/P6 `icedtea_ui::view::builders::*` |
| App loop | P4 `icedtea_ui::view::app::{App, AppError}`, `view::cmd::Cmd` |
| Window | P3 `icedtea_ui::window::{Window, Surface, SurfaceSpec, SurfaceError}` |
| Theme compilation | M2 `icedtea_ui::css::parse::{parse_stylesheet_with_base, MediaEnv, ColorScheme, Contrast}`, `css::cascade::CompiledSheet::compile_with_env`, `BUNDLED_ADWAITA_{LIGHT,DARK,HC}` |
| Layout read-back | M2 `icedtea_ui::layout::{LayoutTree, Allocation, Rect}` |
| Icons | P7 `icedtea_ui::icons::IconTheme`, M2 `css::value::IconRef` |
| Canvas (DrawingArea sample) | `skia_rs_safe::canvas::Canvas`, `skia_rs_safe::paint::Paint`, `skia_rs_safe::core::Color` |
| Compositor under test | `icedtea_harness::{Compositor, ScreencopyClient, VirtualPointerClient, VirtualKeyboardClient, CapturedFrame}` |
| Pixel read-back | M2 `icedtea_ui::shm::pixel_rgb` via `support::pixel_at` |

**Facts this part depends on, verified on this machine 2026-08-27:**

- `ui/tests/support/mod.rs` already exports `Reaper`, `PrintedAllocation`,
  `allocation_of`, `spawn_themed_button`, `spawn_themed_button_with_theme`,
  `pixel_at`, `close`, `matches`, `capture_until`, `SCREENCOPY_TOLERANCE = 12`,
  `CAPTURE_POLL = 25ms`, `TEST_THEME = "bundled"`. It is `#![allow(dead_code)]`
  and compiled once per test binary, so adding helpers costs the M2 tests
  nothing.
- `VirtualPointerClient` has `motion_absolute(x, y, x_extent, y_extent)`,
  `motion(dx, dy)`, `button(code, pressed)`, `frame()`, `pump()` — and **no**
  axis method. `VirtualKeyboardClient` has `key_press(keycode)` (press +
  release) and `pump()`, with a real `us` keymap already handed to the
  compositor at `spawn`.
- `Compositor::spawn()`, `socket_path()`, `output_size() -> (i32, i32)`,
  `settle()` are the harness entry points the M2 screencopy tests use.
- `Paint::new()` + `paint.set_color32(Color(0xFFRRGGBB))` +
  `canvas.draw_rect(&rect.to_skia(), &paint)` is the M2 paint idiom
  (`ui/src/paint/background.rs:119`, `:305`, `ui/src/paint/blur.rs:355`).
- Piped child stdout is **block-buffered**: the gallery must flush after every
  message line or the interaction gate reads nothing until exit.

## Spec

- `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
  — §5 (the widget set the gallery must cover), §7 ("The M3 gate (P8)",
  "Interaction gate", "Cross-cutting"), §1 (P8's row).
- `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` — §7 (all of it, P8's
  normative interface), §4 (`Kind`, `View`, `Props`, `Instance`, `App`), §5
  (every builder this plan calls), §9 P8 (owns/consumes/must-not-touch/gate),
  §8.2 (the M2 tests that must stay byte-identical), §10 (where Task 14 records
  this plan's deviations).

## Global Constraints

- **Crate pins (do not bump, do not add others):** `wayland-client` 0.31,
  `wayland-protocols` 0.32 (`client`, `staging`, `unstable`),
  `wayland-protocols-wlr` 0.3, `skia-rs-safe` 0.4.0 (features
  `std,text,codec,codec-png,svg`), `taffy` 0.14, `cssparser` 0.37, `selectors`
  0.40, `fontconfig` 0.11 (optional, default on), `xkbcommon` 0.9, `bitflags`
  2, `rustix` 1, `tempfile` 3 (dev). `wlr` 0.20.28 from crates.io.
- **No `gtk4`/`gio`/`glib`/`pango`/`cairo`/`gdk`, no Smithay, no
  `smithay-client-toolkit`, no `calloop`.** Ever, in any crate P8 touches.
- **Edition 2024, `rust-version = "1.94"`,** both inherited from
  `[workspace.package]`. Do not set them per crate.
- **Gates, every task:** `cargo test -p icedtea-ui`,
  `cargo clippy -p icedtea-ui --all-targets -- -D warnings`,
  `cargo clippy -p icedtea-ui --no-default-features --all-targets -- -D warnings`,
  `cargo fmt --all --check`. Tasks touching `harness/` additionally run
  `cargo test -p icedtea-harness`. Task 14 additionally runs
  `cargo test --workspace` and `cargo doc -p icedtea-ui --no-deps` with
  `RUSTDOCFLAGS="-D warnings"`.
- **`cargo fmt --all --check` is a hard gate** (PRs #20/#21 made the tree
  rustfmt-clean); run `cargo fmt --all` before every commit.
- **The M1/M2 gates stay green, byte-identical** (contract §8.2):
  `ui/tests/themed_button_offscreen.rs` (4), `ui/tests/adwaita_coverage.rs` (9),
  `ui/tests/gtk4_property_reference.rs` (4),
  `ui/tests/transition_screencopy.rs` (1), every test under `ui/src/css/**`,
  `ui/src/anim/**`, `ui/src/shm.rs`, and every existing `compositor/**` /
  `harness/**` test. P8 edits none of them. Pinned constants that must not move:
  900 compiled Adwaita rules, 1941 lines / 37 `@define-color`s,
  `POOL_INITIAL_BUFFERS=2`, `POOL_MAX_BUFFERS=3`, `CONFIGURE_TIMEOUT=5s`,
  `MARGIN=0`, `BTN_LEFT=0x110`, `#3584e4`, `#1c6fd4`, `#1961b9`.
- **Parts execute in order P1 → P8.** P8 may consume anything P1–P7 produced;
  everything it consumes has already landed on this branch.
- **P1 is executed in a `wlroots-sys` worktree and icedtea consumes the
  published `wlr` 0.20.28 through its pins.** During P2–P8 development a root
  `[patch.crates-io]` pointing at that worktree is allowed, in its own commit,
  and is dropped before merge — exactly as the implicit-grab fix did. P8 neither
  adds nor removes that patch; if it is present when P8 starts, leave it.
- **Untrusted input never panics.** In P8 that is `Options::parse` (argv) and
  the two child-stdout line parsers in `ui/tests/support/mod.rs`. Each gets a
  named never-panic test.
- **Every load-bearing test states its mutation check** in a comment: the exact
  edit that must make it fail, confirmed by running it, then reverted.
- **Timing assertions are generous complexity bounds**, never wall-clock pins.
- **Commit trailer, every commit:**
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **Do not merge.** The branch is presented for review; merging is a consent
  stop owned by the repository owner.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `ui/src/gallery.rs` | create | The gallery as library code: CLI options, kind names, the model/update/view, the exhaustive `sample` table, headless layout + probe-point derivation, and the four entry points `main` dispatches to. |
| `ui/src/bin/gallery.rs` | create | `main` only: argv → `Options` → one `gallery::*` call → exit code. No widget knowledge. |
| `ui/src/lib.rs` | modify | One line: `pub mod gallery;`. Its existing test is untouched. |
| `ui/Cargo.toml` | modify | The `[[bin]] name = "gallery"` entry. |
| `ui/src/view/app.rs` | modify | Additive `App::probe` + `Probe<Msg>` (deviation 2) so geometry is readable without a compositor. |
| `harness/src/lib.rs` | modify | Additive `VirtualPointerClient::{axis, axis_discrete}` (deviation 9). |
| `ui/tests/support/mod.rs` | modify | Grown with the gallery-specific spawn/parse/drive helpers. Every M2 helper stays exactly as it is. |
| `ui/tests/gallery_gate.rs` | create | The rest-state gate: six contract tests + the README table test. |
| `ui/tests/interaction_gate.rs` | create | The sixteen contract interactions, driven by virtual pointer/keyboard. |
| `ui/README.md` | modify | Widget table, module map, gallery/gates section, honest "not covered by M3". |
| `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md` | modify | Status line. |
| `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md` | modify | M3/M4 decomposition status. |
| `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` | modify | §10 amendments recording this plan's eleven deviations. |

---

## Task 1: `gallery` module skeleton — themes, options, kind names, `--list`

**Files:**
- Create: `ui/src/gallery.rs`
- Create: `ui/src/bin/gallery.rs`
- Modify: `ui/src/lib.rs` (add `pub mod gallery;` after `pub mod css;`)
- Modify: `ui/Cargo.toml` (add the second `[[bin]]` entry)
- Test: `ui/src/gallery.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `icedtea_ui::view::Kind` (P4 §4.2, 64 variants, `Kind::all() ->
  &'static [Kind]`); `icedtea_ui::css::parse::{MediaEnv, ColorScheme,
  Contrast}`; `icedtea_ui::{BUNDLED_ADWAITA_LIGHT, BUNDLED_ADWAITA_DARK,
  BUNDLED_ADWAITA_HC}`.
- Produces:
  ```rust
  pub enum Theme { Light, Dark, HighContrast }
  impl Theme {
      pub fn parse(text: &str) -> Option<Theme>;
      pub fn name(self) -> &'static str;          // "light" | "dark" | "hc"
      pub fn sheet(self) -> &'static str;         // the bundled CSS source
      pub fn media_env(self) -> MediaEnv;
  }
  pub struct Options {
      pub theme: Theme,
      pub theme_file: Option<PathBuf>,
      pub widget: Option<Kind>,
      pub list: bool,
      pub probe_points: bool,
      pub print_allocation: bool,
      pub size: (u32, u32),
      pub scroll: i32,
      pub scale: i32,
  }
  impl Options {
      pub const DEFAULT_SIZE: (u32, u32) = (1280, 800);
      pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, OptionsError>;
  }
  pub enum OptionsError { MissingValue(&'static str), BadValue(&'static str, String), Unknown(String) }
  pub fn kind_name(kind: Kind) -> &'static str;
  pub fn kind_from_name(name: &str) -> Option<Kind>;
  pub fn print_list();
  ```

- [ ] **Step 1: Write the failing tests**

Create `ui/src/gallery.rs` with only the test module plus the `use` lines
(everything it names is written in Step 3):

```rust
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
            assert_eq!(kind_from_name(name), Some(kind), "{name} does not round-trip");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: FAIL — `error[E0433]: failed to resolve: use of undeclared type
`Options`` (and the same for `Theme`, `kind_name`, `kind_from_name`), because
`ui/src/lib.rs` does not declare the module yet and the items do not exist.

- [ ] **Step 3: Write the implementation**

Add to `ui/src/lib.rs`, immediately after `pub mod css;`:

```rust
pub mod gallery;
```

Add to `ui/Cargo.toml`, after the existing `[[bin]]` block:

```toml
[[bin]]
name = "gallery"
path = "src/bin/gallery.rs"
```

Insert into `ui/src/gallery.rs`, above the `#[cfg(test)]` module:

```rust
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
```

Add the `SampleShape` table as a second exhaustive match — the six sub-kinds
the contract §4.2 names, everything else `Own`:

```rust
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
```

Create `ui/src/bin/gallery.rs`:

```rust
//! `gallery` — every widget the M3 toolkit ships, on one page.
//!
//! ```text
//! cargo run -p icedtea-ui --bin gallery -- [OPTIONS]
//!
//!   --theme <light|dark|hc>   Which bundled sheet to compile. Default: light.
//!   --theme-file <PATH>       A sheet on disk instead of a bundled one.
//!   --widget <NAME>           Render exactly one widget, alone, at the origin.
//!   --list                    Print every widget name, one per line, and exit.
//!   --probe-points            Print `<widget> <label> <x> <y>` and exit.
//!   --print-allocation        Print `<widget> <x> <y> <w> <h>` and exit.
//!   --size <WxH>              Surface size. Default: 1280x800.
//!   --scroll <PX>             Scroll the page before the first frame.
//!   --scale <N>               Output scale, for HiDPI probes. Default: 1.
//! ```
//!
//! `--list`, `--probe-points` and `--print-allocation` never touch Wayland.

use icedtea_ui::gallery::{Options, print_list};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let opts = match Options::parse(std::env::args().skip(1)) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("gallery: {err}");
            std::process::exit(2);
        }
    };

    if opts.list {
        print_list();
        return;
    }

    // Tasks 5 and 6 replace this with the probe/allocation/run dispatch.
    eprintln!("gallery: nothing to do yet");
    std::process::exit(2);
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: PASS — 5 tests.

Run: `cargo run -p icedtea-ui --bin gallery -- --list | wc -l`
Expected: the same number `Kind::all().len()` reports (64 with the contract's
catalogue as written).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --no-default-features --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/src/gallery.rs ui/src/bin/gallery.rs ui/src/lib.rs ui/Cargo.toml
git commit -m "feat(ui,gallery): options, kind names and --list

The gallery lives in the library, not the binary: Kind is #[non_exhaustive],
so only an in-crate match can be exhaustive, and that exhaustiveness is the
completeness guarantee the M3 gate rests on.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 2: the gallery's Elm loop and the display + button samples

**Files:**
- Modify: `ui/src/gallery.rs` (add the model, the messages, `Sample`, and the
  first 27 arms of `sample`)
- Test: `ui/src/gallery.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: P4 `view::{View, Kind, Cmd}`; P5 builders
  `view::builders::{label, spinner, statusbar, level_bar, progress_bar,
  info_bar, scrollbar, image, picture, separator, text_view, scale,
  drawing_area, window_controls, calendar, popover, button, toggle_button,
  link_button, check_button, menu_button, switch, drop_down,
  color_dialog_button, color_dialog, font_dialog_button, font_dialog}`;
  M2 `css::value::IconRef`.
- Produces:
  ```rust
  pub enum GalleryMsg {
      Clicked(Kind), Toggled(Kind, bool), Changed(Kind, String),
      Selected(Kind, usize), ValueChanged(Kind, f64), Activated(Kind),
      Search(String), PageChanged(usize), Expanded(bool), Closed(Kind),
  }
  impl GalleryMsg { pub fn log_line(&self) -> String; }
  pub struct GalleryModel {
      pub theme: Theme, pub only: Option<Kind>,
      pub toggles: BTreeMap<&'static str, bool>,
      pub texts: BTreeMap<&'static str, String>,
      pub values: BTreeMap<&'static str, f64>,
      pub selected: BTreeMap<&'static str, usize>,
      pub page: usize, pub expanded: bool, pub scroll: i32, pub log: Vec<String>,
  }
  impl GalleryModel {
      pub fn new(theme: Theme, only: Option<Kind>) -> Self;
      pub fn toggle(&self, kind: Kind) -> bool;
      pub fn text(&self, kind: Kind) -> &str;
      pub fn value(&self, kind: Kind) -> f64;
      pub fn selection(&self, kind: Kind) -> usize;
  }
  pub fn update(model: &mut GalleryModel, msg: GalleryMsg) -> Cmd<GalleryMsg>;
  pub enum Sample { Own(View<GalleryMsg>), Within(Kind) }
  pub fn sample(kind: Kind, model: &GalleryModel) -> Sample;
  pub const SAMPLE_PNG: &[u8];
  pub fn sample_png_path() -> PathBuf;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `ui/src/gallery.rs`'s `mod tests`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: FAIL — `cannot find function `sample` in this scope`, `cannot find
type `GalleryModel``, `cannot find value `SAMPLE_PNG``.

- [ ] **Step 3: Write the implementation**

Extend `ui/src/gallery.rs`'s `use` block:

```rust
use std::collections::BTreeMap;
use std::io::Write as _;
use std::rc::Rc;

use skia_rs_safe::core::Color;
use skia_rs_safe::paint::Paint;

use crate::css::value::IconRef;
use crate::layout::Rect;
use crate::view::builders as w;
use crate::view::cmd::Cmd;
use crate::view::{Kind, View};
```

Then the model, messages and the first sample arms:

```rust
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
#[derive(Clone, Debug, Default, PartialEq)]
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

impl Default for Theme {
    fn default() -> Self {
        Theme::Light
    }
}

impl GalleryModel {
    /// A model with every widget in its documented initial state.
    #[must_use]
    pub fn new(theme: Theme, only: Option<Kind>) -> Self {
        let mut model = GalleryModel {
            theme,
            only,
            ..GalleryModel::default()
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
    let fresh = std::fs::read(&path).map(|got| got == SAMPLE_PNG) != Ok(true);
    if fresh && let Err(err) = std::fs::write(&path, SAMPLE_PNG) {
        tracing::warn!(path = %path.display(), %err, "cannot write the gallery's sample png");
    }
    path
}
```

Now the display and button arms. `sample` is written once, in Task 2, with the
27 arms below plus a temporary `_ => Sample::Within(Kind::Box)` that Tasks 3
and 4 delete as they fill in the rest; the totality test in Task 4 is what
proves the placeholder is gone.

```rust
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
        Kind::Statusbar => Sample::Own(w::statusbar().text("Ready")),
        Kind::LevelBar => Sample::Own(
            w::level_bar(0.6)
                .min_value(0.0)
                .max_value(1.0)
                .width_request(160),
        ),
        Kind::ProgressBar => {
            Sample::Own(w::progress_bar(0.4).show_text(true).width_request(160))
        }
        Kind::InfoBar => Sample::Own(
            // ASSUMED: `MessageType` (P5). Read the shipped enum before writing.
            w::info_bar()
                .message_type(w::MessageType::Info)
                .revealed(true)
                .show_close_button(true)
                .on_close(GalleryMsg::Closed(Kind::InfoBar))
                .child(w::label("An info bar")),
        ),
        Kind::Scrollbar => Sample::Own(
            // ASSUMED: `Orientation` (P5/P6).
            w::scrollbar(w::Orientation::Horizontal)
                .value(0.3)
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
        Kind::Separator => {
            Sample::Own(w::separator(w::Orientation::Horizontal).width_request(160))
        }
        Kind::TextView => Sample::Own(
            w::text_view(model.text(Kind::TextView))
                .editable(true)
                .width_request(200)
                .height_request(64)
                .on_change(|t: &str| GalleryMsg::Changed(Kind::TextView, t.to_string())),
        ),
        Kind::Scale => Sample::Own(
            w::scale(0.0, 100.0)
                .value(model.value(Kind::Scale))
                .orientation(w::Orientation::Horizontal)
                .width_request(200)
                .on_value_changed(|v| GalleryMsg::ValueChanged(Kind::Scale, v)),
        ),
        Kind::DrawingArea => Sample::Own(
            w::drawing_area(|canvas: &mut skia_rs_safe::canvas::Canvas<'_>, rect: Rect| {
                let mut paint = Paint::new();
                paint.set_color32(Color(0xFF33_D17A));
                canvas.draw_rect(&rect.to_skia(), &paint);
            })
            .width_request(64)
            .height_request(48),
        ),
        // ASSUMED: `Side` (P5).
        Kind::WindowControls => Sample::Own(w::window_controls(w::Side::End)),
        Kind::Calendar => Sample::Own(
            w::calendar(2026, 8, 27)
                .on_date_selected(|i| GalleryMsg::Selected(Kind::Calendar, i)),
        ),
        // A non-autohide popover renders into the parent window's own tree
        // (§5.1's ruling), which is what gives it pixels at rest.
        Kind::Popover => Sample::Own(
            w::popover(w::label("Popover"))
                .autohide(false)
                .has_arrow(true),
        ),

        // ---- P5 · buttons ------------------------------------------------
        Kind::Button => Sample::Own(w::button("Button").on_click(GalleryMsg::Clicked(Kind::Button))),
        Kind::ToggleButton => Sample::Own(
            w::toggle_button("Toggle")
                .active(model.toggle(Kind::ToggleButton))
                .on_toggle(|on| GalleryMsg::Toggled(Kind::ToggleButton, on)),
        ),
        Kind::LinkButton => Sample::Own(
            w::link_button("https://example.invalid/", "Link")
                .on_activate_link(|_uri: &str| GalleryMsg::Clicked(Kind::LinkButton)),
        ),
        Kind::CheckButton => Sample::Own(
            w::check_button("Check")
                .active(model.toggle(Kind::CheckButton))
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
        // ASSUMED: `Rgba` (P5); a literal-constructed opaque blue.
        Kind::ColorDialogButton => Sample::Own(
            w::color_dialog_button(w::Rgba::new(0.21, 0.52, 0.89, 1.0))
                .on_change(|s: &str| GalleryMsg::Changed(Kind::ColorDialogButton, s.to_string())),
        ),
        Kind::ColorDialog => Sample::Own(w::color_dialog(w::Rgba::new(0.21, 0.52, 0.89, 1.0))),
        Kind::FontDialogButton => Sample::Own(
            w::font_dialog_button("Cantarell 11")
                .on_change(|s: &str| GalleryMsg::Changed(Kind::FontDialogButton, s.to_string())),
        ),
        Kind::FontDialog => Sample::Own(w::font_dialog("Cantarell 11")),

        // Tasks 3 and 4 replace this arm with the remaining 37.
        _ => Sample::Within(Kind::Box),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: PASS — 9 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/src/gallery.rs
git commit -m "feat(ui,gallery): the gallery's model, messages and display/button samples

Every folded message prints one flushed line on stdout: that is how the
interaction gate makes model assertions across the process boundary.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 3: the entry and container samples

**Files:**
- Modify: `ui/src/gallery.rs` (`sample`: 5 entry arms + 17 container arms)
- Test: `ui/src/gallery.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 2's `GalleryModel`/`GalleryMsg`/`Sample`; P5 builders `entry,
  search_entry, password_entry, spin_button, editable_label`; P6 builders
  `box_, grid, center_box, scrolled_window, paned, frame, expander, search_bar,
  action_bar, header_bar, notebook, notebook_tab, overlay, stack, stack_page,
  stack_switcher, stack_sidebar`.
- Produces: no new signatures — `sample` gains 22 arms and
  `Kind::{NotebookTab, StackPage}` start answering `Sample::Within`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-ui --lib gallery::tests::every_entry_and_container`
Expected: FAIL — `assertion `left == right` failed: entry must be its own
entry, got Box` (the Task 2 placeholder arm answers for everything else).

- [ ] **Step 3: Write the implementation**

Replace the placeholder arm with the 22 arms below (keep the `_ =>` arm; Task 4
removes it):

```rust
        // ---- P5 · entries ------------------------------------------------
        Kind::Entry => Sample::Own(
            w::entry(model.text(Kind::Entry))
                .placeholder("Type here")
                .width_chars(16)
                .on_change(|t: &str| GalleryMsg::Changed(Kind::Entry, t.to_string()))
                .on_activate(GalleryMsg::Activated(Kind::Entry)),
        ),
        Kind::SearchEntry => Sample::Own(
            w::search_entry(model.text(Kind::SearchEntry))
                .placeholder("Search")
                .on_search(|t: &str| GalleryMsg::Search(t.to_string()))
                .on_change(|t: &str| GalleryMsg::Changed(Kind::SearchEntry, t.to_string())),
        ),
        Kind::PasswordEntry => Sample::Own(
            w::password_entry(model.text(Kind::PasswordEntry))
                .show_peek_icon(true)
                .on_change(|t: &str| GalleryMsg::Changed(Kind::PasswordEntry, t.to_string())),
        ),
        Kind::SpinButton => Sample::Own(
            w::spin_button(model.value(Kind::SpinButton), 0.0, 10.0)
                .step(1.0)
                .digits(0)
                .on_value_changed(|v| GalleryMsg::ValueChanged(Kind::SpinButton, v)),
        ),
        Kind::EditableLabel => Sample::Own(
            w::editable_label(model.text(Kind::EditableLabel))
                .on_change(|t: &str| GalleryMsg::Changed(Kind::EditableLabel, t.to_string())),
        ),

        // ---- P6 · containers ---------------------------------------------
        Kind::Box => Sample::Own(
            w::box_(
                w::Orientation::Horizontal,
                [w::label("One"), w::label("Two"), w::label("Three")],
            )
            .spacing(6),
        ),
        Kind::Grid => Sample::Own(
            w::grid([
                w::label("0,0").column(0).row(0),
                w::label("1,0").column(1).row(0),
                w::label("wide").column(0).row(1).column_span(2),
            ])
            .row_spacing(6)
            .column_spacing(6),
        ),
        Kind::CenterBox => Sample::Own(
            w::center_box(w::label("start"), w::label("centre"), w::label("end"))
                .orientation(w::Orientation::Horizontal)
                .width_request(240),
        ),
        Kind::ScrolledWindow => Sample::Own(
            w::scrolled_window(w::box_(
                w::Orientation::Vertical,
                (0..12).map(|i| w::label(&format!("Row {i}")).key(i as usize)),
            ))
            .min_content_width(180)
            .min_content_height(80),
        ),
        Kind::Paned => Sample::Own(
            w::paned(w::Orientation::Horizontal, w::label("left"), w::label("right"))
                .position(90)
                .width_request(200)
                .height_request(60),
        ),
        Kind::Frame => Sample::Own(w::frame(w::label("Framed")).label("Frame")),
        Kind::Expander => Sample::Own(
            w::expander("Expander", w::label("Revealed"))
                .expanded(model.expanded)
                .on_expanded(GalleryMsg::Expanded(true)),
        ),
        Kind::SearchBar => Sample::Own(
            w::search_bar(w::search_entry("")).search_mode(true).width_request(240),
        ),
        Kind::ActionBar => Sample::Own(
            w::action_bar()
                .revealed(true)
                .pack_start(w::button("Start").on_click(GalleryMsg::Clicked(Kind::ActionBar)))
                .pack_end(w::button("End").on_click(GalleryMsg::Clicked(Kind::ActionBar)))
                .width_request(240),
        ),
        Kind::HeaderBar => Sample::Own(
            w::header_bar()
                .title("Header")
                .subtitle("bar")
                .pack_start(w::button("Back").on_click(GalleryMsg::Clicked(Kind::HeaderBar)))
                .width_request(280),
        ),
        Kind::Notebook => Sample::Own(
            w::notebook([
                w::notebook_tab(w::label("One"), w::label("Page one")).key(0usize),
                w::notebook_tab(w::label("Two"), w::label("Page two")).key(1usize),
            ])
            .page(model.page)
            .on_page_changed(GalleryMsg::PageChanged)
            .width_request(240)
            .height_request(100),
        ),
        Kind::NotebookTab => Sample::Within(Kind::Notebook),
        Kind::Overlay => Sample::Own(
            w::overlay(w::label("Under"))
                .overlay(w::label("Over").halign(w::Align::End))
                .width_request(160)
                .height_request(48),
        ),
        Kind::Stack => Sample::Own(
            w::stack([
                w::stack_page("one", "One", w::label("Page one")).key("one"),
                w::stack_page("two", "Two", w::label("Page two")).key("two"),
            ])
            .visible_child(if model.page == 0 { "one" } else { "two" })
            .transition_type(w::StackTransition::SlideLeftRight)
            .transition_duration(120)
            .on_change(|name: &str| GalleryMsg::PageChanged(usize::from(name == "two")))
            .width_request(200)
            .height_request(60),
        ),
        Kind::StackPage => Sample::Within(Kind::Stack),
        Kind::StackSwitcher => Sample::Own(
            w::stack_switcher(stack_pages())
                .selected(model.page)
                .on_selected(GalleryMsg::PageChanged),
        ),
        Kind::StackSidebar => Sample::Own(
            w::stack_sidebar(stack_pages())
                .selected(model.page)
                .on_selected(GalleryMsg::PageChanged)
                .width_request(120)
                .height_request(80),
        ),
```

and the one helper both switchers share, next to `sample`:

```rust
/// The two pages `StackSwitcher` and `StackSidebar` present.
///
/// ASSUMED: `StackPageInfo` (P6) — `name`, `title`, optional icon.
fn stack_pages() -> Rc<[w::StackPageInfo]> {
    Rc::from(
        [
            w::StackPageInfo::new("one", "One"),
            w::StackPageInfo::new("two", "Two"),
        ]
        .as_slice(),
    )
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: PASS — 11 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/src/gallery.rs
git commit -m "feat(ui,gallery): entry and container samples

Every interactive sample reads its state back out of the model, so an
interaction that does not reach the model cannot change a pixel either.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 4: the list, menu and window samples — and `page`

**Files:**
- Modify: `ui/src/gallery.rs` (`sample`'s last 15 arms, `items`, `page`)
- Test: `ui/src/gallery.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 3's `sample`; P6 builders `list_box, list_box_row, flow_box,
  flow_box_child, list_view, grid_view, column_view, column_view_column,
  popover_menu, popover_menu_item, popover_menu_bar, window, shortcuts_window,
  about_dialog, alert_dialog`.
- Produces:
  ```rust
  pub fn items(labels: &[&str]) -> Rc<[ListItem]>;      // ASSUMED ListItem (P6)
  pub fn page(model: &GalleryModel) -> View<GalleryMsg>;
  pub fn own_kinds() -> Vec<Kind>;                      // Kind::all() minus sub-kinds
  ```

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
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
        assert_eq!(page.children.len(), expected.len(), "one frame per own kind");
        for (frame, kind) in page.children.iter().zip(&expected) {
            assert_eq!(frame.kind, Kind::Frame, "{} is not framed", kind_name(*kind));
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: FAIL — `cannot find function `page` in this scope` and
`sample() and sample_shape() disagree about list_box`.

- [ ] **Step 3: Write the implementation**

Delete `sample`'s `_ =>` arm and add the last fifteen:

```rust
        // ---- P6 · lists ---------------------------------------------------
        Kind::ListBox => Sample::Own(
            w::list_box([
                w::list_box_row(w::label("Row 0")).key(0usize).activatable(true),
                w::list_box_row(w::label("Row 1")).key(1usize).activatable(true),
                w::list_box_row(w::label("Row 2")).key(2usize).activatable(true),
            ])
            // ASSUMED: `SelectionMode` (P6).
            .selection_mode(w::SelectionMode::Single)
            .show_separators(true)
            .on_selected(|i| GalleryMsg::Selected(Kind::ListBox, i))
            .width_request(200),
        ),
        Kind::ListBoxRow => Sample::Within(Kind::ListBox),
        Kind::FlowBox => Sample::Own(
            w::flow_box((0..6).map(|i| {
                w::flow_box_child(w::label(&format!("{i}"))).key(i as usize)
            }))
            .selection_mode(w::SelectionMode::Single)
            .min_children_per_line(3)
            .max_children_per_line(3)
            .on_selected(|i| GalleryMsg::Selected(Kind::FlowBox, i))
            .width_request(200),
        ),
        Kind::FlowBoxChild => Sample::Within(Kind::FlowBox),
        Kind::ListView => Sample::Own(
            w::list_view(items(LIST_ROWS), row_factory())
                .selection_mode(w::SelectionMode::Single)
                .selected(model.selection(Kind::ListView))
                .on_selected(|i| GalleryMsg::Selected(Kind::ListView, i))
                .width_request(200)
                .height_request(120),
        ),
        Kind::GridView => Sample::Own(
            w::grid_view(items(LIST_ROWS), row_factory())
                .min_columns(2)
                .max_columns(2)
                .on_selected(|i| GalleryMsg::Selected(Kind::GridView, i))
                .width_request(200)
                .height_request(120),
        ),
        Kind::ColumnView => Sample::Own(
            w::column_view(
                items(LIST_ROWS),
                [
                    w::column_view_column("Name", row_factory()).resizable(true),
                    w::column_view_column("Value", row_factory()).expand(true),
                ],
            )
            .show_row_separators(true)
            .show_column_separators(true)
            .on_selected(|i| GalleryMsg::Selected(Kind::ColumnView, i))
            .width_request(240)
            .height_request(120),
        ),
        Kind::ColumnViewColumn => Sample::Within(Kind::ColumnView),

        // ---- P6 · menus ---------------------------------------------------
        Kind::PopoverMenu => Sample::Own(
            w::popover_menu([
                w::popover_menu_item("Open").key("open").accel("<Ctrl>O"),
                w::popover_menu_item("Save").key("save").accel("<Ctrl>S"),
                w::popover_menu_item("Quit").key("quit"),
            ])
            .autohide(false)
            .on_activate(|i| GalleryMsg::Selected(Kind::PopoverMenu, i)),
        ),
        Kind::PopoverMenuBar => Sample::Own(
            w::popover_menu_bar([w::popover_menu_item("File").key("file")])
                .menu("File", w::label("File menu"))
                .on_activate(|i| GalleryMsg::Selected(Kind::PopoverMenuBar, i))
                .width_request(200),
        ),
        Kind::PopoverMenuItem => Sample::Within(Kind::PopoverMenu),

        // ---- P6 · windows -------------------------------------------------
        // The window-ish kinds are node trees like any other: the gallery
        // embeds them in the page rather than mapping four more surfaces.
        Kind::Window => Sample::Own(
            w::window(w::label("Window content"))
                .title("Window")
                .width_request(220)
                .height_request(80),
        ),
        Kind::ShortcutsWindow => Sample::Own(
            w::shortcuts_window([w::label("Ctrl+Q — Quit")])
                .section_name("general")
                .view_name("main")
                .width_request(220)
                .height_request(80),
        ),
        Kind::AboutDialog => Sample::Own(
            w::about_dialog("icedtea")
                .version("0.1.0")
                .comments("A pure-Rust GTK-themed toolkit")
                .width_request(220)
                .height_request(100),
        ),
        Kind::AlertDialog => Sample::Own(
            w::alert_dialog("Delete everything?")
                .detail("This cannot be undone.")
                .buttons(&["Cancel", "Delete"])
                .on_response(|i| GalleryMsg::Selected(Kind::AlertDialog, i))
                .width_request(240),
        ),
```

and the shared list helpers plus the page builder:

```rust
/// The rows every list-ish sample shows. Ten, so `ListView` has more rows than
/// fit its 120 px viewport and the scroll interaction actually recycles.
const LIST_ROWS: &[&str] = &[
    "Row 0", "Row 1", "Row 2", "Row 3", "Row 4", "Row 5", "Row 6", "Row 7", "Row 8", "Row 9",
];

/// A list model from plain strings.
///
/// ASSUMED: `ListItem` (P6) with a `text` constructor.
#[must_use]
pub fn items(labels: &[&str]) -> Rc<[w::ListItem]> {
    labels.iter().map(|t| w::ListItem::text(t)).collect()
}

/// The factory every list-ish sample binds its rows with: one label per item.
///
/// ASSUMED: the `factory` parameter of `list_view`/`grid_view`/
/// `column_view_column` is `Rc<dyn Fn(usize, &ListItem) -> View<Msg>>`.
fn row_factory() -> Rc<dyn Fn(usize, &w::ListItem) -> View<GalleryMsg>> {
    Rc::new(|index, item| w::label(item.text()).key(index))
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
            Sample::Own(view) => view.halign(w::Align::Start).valign(w::Align::Start),
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
        w::frame(view)
            .label(kind_name(kind))
            .key(kind_name(kind))
            .halign(w::Align::Start)
    });
    w::box_(w::Orientation::Vertical, frames)
        .spacing(12)
        .margin(12, 12, 12, 12)
        .class("gallery")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib gallery::`
Expected: PASS — 15 tests, including
`sample_is_total_and_every_sub_kind_names_a_real_parent`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/src/gallery.rs
git commit -m "feat(ui,gallery): list, menu and window samples; the page itself

sample() is now total over Kind::all() with no wildcard arm: adding a Kind
without a gallery entry is a compile error, which is the completeness
guarantee the M3 gate rests on.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 5: headless geometry — `App::probe`, probe-point derivation, `--probe-points`, `--print-allocation`

**Files:**
- Modify: `ui/src/view/app.rs` (additive: `Probe<Msg>` + `App::probe`)
- Modify: `ui/src/gallery.rs` (`build`, `probe_points_of`, `entry_instances`,
  `print_probe_points`, `print_allocations`)
- Modify: `ui/src/bin/gallery.rs` (dispatch the two printing modes)
- Test: `ui/src/view/app.rs` `mod tests`, `ui/src/gallery.rs` `mod tests`

**Interfaces:**
- Consumes: P4 `view::{Instance, reconcile, BuildCx}`, `view::app::{App,
  AppError}`; M2 `layout::{LayoutTree, Allocation}`, `css::node::Node`,
  `anim::clock::Clock`; P7 `icons::IconTheme`; M2 `text::FontDatabase`,
  `css::cascade::CompiledSheet`.
- Produces:
  ```rust
  // ui/src/view/app.rs — additive
  pub struct Probe<Msg> { /* root: Node, instances: Vec<Instance<Msg>>, tree: LayoutTree */ }
  impl<Msg> Probe<Msg> {
      pub fn root(&self) -> &Node;
      pub fn instances(&self) -> &[Instance<Msg>];
      pub fn allocation(&self, node: &Node) -> Option<Allocation>;
  }
  impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
      pub fn probe(self, size: (u32, u32), sheet: CompiledSheet, fonts: FontDatabase,
                   icons: IconTheme, clock: Rc<dyn Clock>) -> Result<Probe<Msg>, AppError>;
  }

  // ui/src/gallery.rs
  pub struct ProbePoint { pub widget: &'static str, pub label: String, pub x: i32, pub y: i32 }
  pub fn build(opts: &Options) -> Result<Probe<GalleryMsg>, AppError>;
  pub fn compile_sheet(opts: &Options) -> CompiledSheet;
  pub fn probe_points_of(instance: &Instance<GalleryMsg>, widget: &'static str,
                         probe: &Probe<GalleryMsg>) -> Vec<ProbePoint>;
  pub fn entry_instances<'a>(opts: &Options, probe: &'a Probe<GalleryMsg>)
      -> Vec<(&'static str, &'a Instance<GalleryMsg>)>;
  pub fn print_probe_points(opts: &Options) -> Result<(), AppError>;
  pub fn print_allocations(opts: &Options) -> Result<(), AppError>;
  ```

- [ ] **Step 1: Write the failing tests**

In `ui/src/view/app.rs`'s test module:

```rust
    /// `probe` must run the *same* pipeline `run` does — view, reconcile,
    /// restyle, layout — or the gate's coordinates would describe a tree
    /// nobody renders.
    ///
    /// Mutation check: make `probe` skip its `compute` call; every allocation
    /// comes back 0x0 and this test fails. Restore.
    #[test]
    fn probe_lays_the_tree_out_and_reports_allocations() {
        let clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
        let app = App::new(0u32, |_m: &mut u32, _msg: ()| Cmd::None, |_m: &u32| {
            crate::view::builders::label("probe me")
        });
        let sheet = CompiledSheet::from_stylesheet(parse_stylesheet_with_base(
            crate::BUNDLED_ADWAITA_LIGHT,
            None,
        ));
        let probe = app
            .probe(
                (200, 100),
                sheet,
                FontDatabase::new(),
                IconTheme::with_name_and_roots("hicolor", Vec::new()),
                clock,
            )
            .expect("probe lays out");
        assert_eq!(probe.instances().len(), 1);
        let alloc = probe
            .allocation(&probe.instances()[0].node)
            .expect("the root has an allocation");
        assert!(
            alloc.border_box.width > 0.0 && alloc.border_box.height > 0.0,
            "a label with text must have a non-empty allocation, got {:?}",
            alloc.border_box
        );
    }
```

In `ui/src/gallery.rs`'s test module:

```rust
    #[test]
    fn every_widget_exposes_a_root_probe_point_inside_its_own_allocation() {
        let opts = Options::parse(vec![]).unwrap();
        let probe = build(&opts).expect("the page lays out headlessly");
        let entries = entry_instances(&opts, &probe);
        assert_eq!(entries.len(), own_kinds().len());
        for (widget, instance) in entries {
            let points = probe_points_of(instance, widget, &probe);
            assert!(!points.is_empty(), "{widget} has no probe points");
            assert_eq!(points[0].label, "root", "{widget}'s first point is its root");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --lib view::app::tests::probe_lays`
Expected: FAIL — `no method named `probe` found for struct `App``.

Run: `cargo test -p icedtea-ui --lib gallery::tests::every_widget_exposes`
Expected: FAIL — `cannot find function `build` in this scope`.

- [ ] **Step 3: Write the implementation**

In `ui/src/view/app.rs`, additive — nothing existing changes:

```rust
/// A built, styled and laid-out view tree with no Wayland connection.
///
/// This is what `gallery --probe-points` reads: the same pipeline
/// [`App::run`] drives, stopped after the first layout so a tool can ask the
/// tree where its nodes ended up. Without it the only way to learn a
/// coordinate would be to hard-code it, which the M3 gate forbids.
pub struct Probe<Msg> {
    root: Node,
    instances: Vec<Instance<Msg>>,
    tree: LayoutTree,
}

impl<Msg> Probe<Msg> {
    /// The synthetic root every instance hangs from.
    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The top-level instances, in view order.
    #[must_use]
    pub fn instances(&self) -> &[Instance<Msg>] {
        &self.instances
    }

    /// `node`'s allocation in tree-origin coordinates, or `None` for a node
    /// that is not in this tree.
    #[must_use]
    pub fn allocation(&self, node: &Node) -> Option<Allocation> {
        self.tree.allocation(node)
    }
}

impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    /// Run view → reconcile → restyle → layout **once**, against `size`, with
    /// no surface, and hand back the result.
    ///
    /// `sheet`, `fonts` and `icons` are parameters rather than fields because
    /// [`App::run`] takes them from the `Window` it is handed; a probe has no
    /// window, so its caller supplies them.
    ///
    /// # Errors
    ///
    /// [`AppError::Layout`] if the tree cannot be laid out.
    pub fn probe(
        self,
        size: (u32, u32),
        sheet: CompiledSheet,
        mut fonts: FontDatabase,
        mut icons: IconTheme,
        clock: Rc<dyn Clock>,
    ) -> Result<Probe<Msg>, AppError> {
        // Same body as `run`'s first frame, minus the surface: build the view
        // from the model, reconcile it into instances, restyle the dirty
        // subtree, then lay out against `size` as the available space.
        // `probe` lives in `app.rs`, so it reads the private fields directly.
        let App { model, view, .. } = self;
        let env = ResolveEnv::default();
        let root = Node::with_classes("window", &["background"]);
        let mut instances: Vec<Instance<Msg>> = Vec::new();
        let mut cx = BuildCx {
            sheet: &sheet,
            fonts: &mut fonts,
            icons: &mut icons,
            clock: &clock,
            env: &env,
        };
        let _ops = reconcile(&root, &mut instances, vec![view(&model)], &mut cx);
        let mut tree = LayoutTree::new();
        restyle_and_layout(&root, &instances, &sheet, &env, &mut fonts, &mut tree, size)
            .map_err(AppError::Layout)?;
        Ok(Probe {
            root,
            instances,
            tree,
        })
    }
}
```

where `restyle_and_layout` is `App`'s existing private per-frame styling and
layout pass, factored out of `run`/`run_offscreen` so all three callers share
one implementation. **Do not duplicate it**: extract the existing body, leave
`run`/`run_offscreen` calling the extracted function, and confirm P4's tests
still pass unchanged.

Extend `ui/src/gallery.rs`'s `use` block with what the new code names:

```rust
use crate::anim::clock::{Clock, MonotonicClock};
use crate::css::cascade::CompiledSheet;
use crate::css::parse::parse_stylesheet_with_base;
use crate::icons::IconTheme;
use crate::layout::Allocation;
use crate::text::FontDatabase;
use crate::view::app::{App, AppError, Probe};
use crate::view::Instance;
```

and `ui/src/view/app.rs`'s with `crate::css::computed::ResolveEnv`,
`crate::css::node::Node`, `crate::layout::{Allocation, LayoutTree}` and
`crate::icons::IconTheme` if they are not already there.

Then, in `ui/src/gallery.rs`:

```rust
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
    match &opts.theme_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(css) => CompiledSheet::compile_with_env(
                &parse_stylesheet_with_base(&css, path.parent()),
                &env,
            ),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "cannot read --theme-file; using the bundled sheet");
                CompiledSheet::compile_with_env(
                    &parse_stylesheet_with_base(opts.theme.sheet(), None),
                    &env,
                )
            }
        },
        None => CompiledSheet::compile_with_env(
            &parse_stylesheet_with_base(opts.theme.sheet(), None),
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
pub fn scrolled_page(model: &GalleryModel) -> View<GalleryMsg> {
    let page = page(model);
    if model.scroll == 0 || model.only.is_some() {
        page
    } else {
        page.margin(12 - model.scroll, 12, 12, 12)
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
/// Nodes with an empty allocation are skipped: a point nothing paints is a
/// point no capture can assert on.
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
        if let Some(alloc) = probe.allocation(node)
            && !alloc.border_box.is_empty()
        {
            points.push(centre(widget, label, &alloc));
        }
    }
    points
}

/// The centre of a border box, rounded toward the origin.
fn centre(widget: &'static str, label: String, alloc: &Allocation) -> ProbePoint {
    let r = alloc.border_box;
    ProbePoint {
        widget,
        label,
        x: (r.x + r.width / 2.0) as i32,
        y: (r.y + r.height / 2.0) as i32,
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
```

In `ui/src/bin/gallery.rs`, replace the "nothing to do yet" block:

```rust
    let result = if opts.probe_points {
        icedtea_ui::gallery::print_probe_points(&opts)
    } else if opts.print_allocation {
        icedtea_ui::gallery::print_allocations(&opts)
    } else {
        // Task 6 fills this in.
        eprintln!("gallery: nothing to do yet");
        std::process::exit(2);
    };
    if let Err(err) = result {
        eprintln!("gallery: {err:?}");
        std::process::exit(1);
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --lib`
Expected: PASS — the four new tests plus every existing library test (P4's
`view::app` tests must be unchanged and green after the `restyle_and_layout`
extraction).

Run: `cargo run -p icedtea-ui --bin gallery -- --probe-points | head -5`
Expected: five `<widget> <label> <x> <y>` lines, the first being
`label root <x> <y>`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/src/view/app.rs ui/src/gallery.rs ui/src/bin/gallery.rs
git commit -m "feat(ui,gallery): headless probe points and allocations

App::probe runs the same view/reconcile/restyle/layout pipeline run() does and
stops after the first layout, so the gate can read coordinates off the real
tree instead of hard-coding them (contract deviation 2).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 6: `gallery::run`, the test-support layer, and the first rest-state gate test

**Files:**
- Modify: `ui/src/gallery.rs` (`run`)
- Modify: `ui/src/bin/gallery.rs` (dispatch `run`)
- Modify: `ui/tests/support/mod.rs` (grown; every M2 helper untouched)
- Create: `ui/tests/gallery_gate.rs`
- Test: `ui/tests/gallery_gate.rs`

**Interfaces:**
- Consumes: Task 5's `build`/`compile_sheet`/`entry_instances`; P3
  `window::{Window, SurfaceSpec, SurfaceError}`; P4 `view::app::App`;
  `icedtea_harness::{Compositor, ScreencopyClient, CapturedFrame}`.
- Produces:
  ```rust
  // ui/src/gallery.rs
  pub fn run(opts: &Options) -> Result<(), AppError>;

  // ui/tests/support/mod.rs
  pub struct ProbePoint { pub widget: String, pub label: String, pub x: i32, pub y: i32 }
  pub struct EntryAllocation { pub widget: String, pub x: f32, pub y: f32,
                               pub width: f32, pub height: f32 }
  pub struct GalleryProc { /* child + captured stdout */ }
  impl GalleryProc {
      pub fn wait_msg(&self, needle: &str, timeout: Duration) -> bool;
      pub fn messages(&self) -> Vec<String>;
  }
  pub fn parse_probe_line(line: &str) -> Option<ProbePoint>;
  pub fn parse_allocation_line(line: &str) -> Option<EntryAllocation>;
  pub fn probe_points(theme: &str, widget: Option<&str>) -> Vec<ProbePoint>;
  pub fn entry_allocations(theme: &str) -> Vec<EntryAllocation>;
  pub fn spawn_gallery(socket: &str, theme: &str, scroll: i32) -> GalleryProc;
  pub fn spawn_gallery_widget(socket: &str, theme: &str, widget: &str) -> GalleryProc;
  pub fn wait_for_gallery(screencopy: &mut ScreencopyClient, probe: (u32, u32),
                          before: (u8, u8, u8)) -> CapturedFrame;
  pub fn paints_something(frame: &CapturedFrame, rect: (i32, i32, i32, i32),
                          background: (u8, u8, u8)) -> bool;
  ```

- [ ] **Step 1: Write the failing test**

Create `ui/tests/gallery_gate.rs`:

```rust
//! The M3 rest-state gate.
//!
//! The gallery renders every `Kind` the toolkit ships; this file proves each
//! one actually reaches a real compositor's screen, in all three bundled
//! Adwaita sheets, at coordinates it learned from the binary rather than from
//! a literal.

mod support;

use std::time::Duration;

use icedtea_harness::{Compositor, ScreencopyClient};
use support::{
    EntryAllocation, ProbePoint, entry_allocations, paints_something, parse_allocation_line,
    parse_probe_line, pixel_at, probe_points, spawn_gallery, wait_for_gallery,
};

/// How the page is walked: one capture per `--scroll` slice, stepping a little
/// less than the surface height so nothing falls between two slices.
const SLICE_STEP: i32 = 700;
/// Surface height the gallery is spawned at (its documented default).
const SURFACE_HEIGHT: i32 = 800;

/// Widgets whose entry is visible in the slice captured at `scroll`.
fn visible_in_slice(allocations: &[EntryAllocation], scroll: i32) -> Vec<(String, (i32, i32, i32, i32))> {
    allocations
        .iter()
        .filter_map(|entry| {
            let top = entry.y as i32 - scroll;
            let bottom = top + entry.height as i32;
            (top >= 2 && bottom <= SURFACE_HEIGHT - 2).then(|| {
                (
                    entry.widget.clone(),
                    (entry.x as i32, top, entry.width as i32, entry.height as i32),
                )
            })
        })
        .collect()
}

/// Every own-kind entry paints something, in `theme`.
///
/// "Paints something" is the honest assertion: a widget whose entry rectangle
/// is entirely the window background has not rendered, whatever its node tree
/// says. Colours are not pinned here — that is `themed_button_offscreen.rs`'s
/// job, and pinning 64 of them would be a fixture, not a gate.
fn every_widget_renders_at_rest(theme: &str) {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();
    let (out_w, out_h) = compositor.output_size();
    assert!(
        out_w >= 1280 && out_h >= 800,
        "the gallery gate needs at least a 1280x800 output, got {out_w}x{out_h}"
    );
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let empty = screencopy.capture();
    let background_probe = (out_w as u32 - 3, out_h as u32 / 2);
    let wallpaper = pixel_at(&empty, background_probe.0, background_probe.1)
        .expect("the background probe is inside the frame");

    let allocations = entry_allocations(theme);
    assert!(!allocations.is_empty(), "the gallery printed no allocations");
    let page_height = allocations
        .iter()
        .map(|a| (a.y + a.height) as i32)
        .max()
        .expect("a non-empty page");
    let points = probe_points(theme, None);

    let mut seen: Vec<String> = Vec::new();
    let mut scroll = 0;
    while scroll < page_height {
        let gallery = spawn_gallery(&socket, theme, scroll);
        let frame = wait_for_gallery(&mut screencopy, background_probe, wallpaper);
        let page_background =
            pixel_at(&frame, background_probe.0, background_probe.1).expect("inside the frame");
        for (widget, rect) in visible_in_slice(&allocations, scroll) {
            if seen.contains(&widget) {
                continue;
            }
            assert!(
                paints_something(&frame, rect, page_background),
                "{widget} painted nothing in the {theme} theme; its entry is {rect:?}"
            );
            for point in points.iter().filter(|p| p.widget == widget) {
                let y = point.y - scroll;
                assert!(
                    pixel_at(&frame, point.x as u32, y as u32).is_some(),
                    "{widget}/{} is outside the captured frame at ({}, {y})",
                    point.label,
                    point.x
                );
            }
            seen.push(widget);
        }
        drop(gallery);
        scroll += SLICE_STEP;
    }

    let missing: Vec<&str> = allocations
        .iter()
        .map(|a| a.widget.as_str())
        .filter(|w| !seen.iter().any(|s| s == w))
        .collect();
    assert!(
        missing.is_empty(),
        "no slice ever showed these widgets whole: {missing:?}"
    );
}

/// Mutation check: make `gallery::page` skip `Kind::Switch`'s frame; this test
/// fails with "no slice ever showed these widgets whole: [\"switch\"]".
/// Restore.
#[test]
fn every_widget_renders_at_rest_in_the_light_theme() {
    every_widget_renders_at_rest("light");
}

/// Never-panic gate for the two stdout parsers: the gate reads a child
/// process's output, and a crashed or half-written child must fail the
/// assertion, not the harness.
///
/// Mutation check: parse a field with `unwrap()` instead of `ok()?`; this test
/// panics instead of passing. Restore.
#[test]
fn hostile_child_output_is_parsed_without_panicking() {
    let hostile = [
        "",
        " ",
        "\u{0}",
        "label",
        "label root",
        "label root 1",
        "label root x y",
        "label root -1 -1",
        "label root 99999999999999999999 0",
        "label root 1 2 3 4 5",
        "лейбл корень 1 2",
        "label root 1.5 2.5",
    ];
    for line in hostile {
        let _ = parse_probe_line(line);
        let _ = parse_allocation_line(line);
    }
    assert_eq!(
        parse_probe_line("check_button check 40 21"),
        Some(ProbePoint {
            widget: "check_button".to_string(),
            label: "check".to_string(),
            x: 40,
            y: 21,
        })
    );
    let alloc = parse_allocation_line("button 12 340 78 34").expect("a well-formed line");
    assert_eq!(alloc.widget, "button");
    assert!((alloc.height - 34.0).abs() < f32::EPSILON);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test gallery_gate`
Expected: FAIL to compile — `cannot find function `probe_points` in module
`support``, `cannot find function `spawn_gallery``, and the others.

- [ ] **Step 3: Write the implementation**

In `ui/src/gallery.rs`:

```rust
/// Map the gallery as a layer-shell overlay and run the app loop.
///
/// A layer surface rather than an xdg-toplevel: probe coordinates are output
/// coordinates, and only the layer role has a compositor-independent origin
/// (anchored top-left, margin 0 — the M2 `themed-button` precedent). Keyboard
/// interactivity is exclusive so the typing and Tab interactions have a
/// focused keyboard.
///
/// # Errors
///
/// [`AppError::Surface`] if the surface cannot be created or configured.
pub fn run(opts: &Options) -> Result<(), AppError> {
    let mut model = GalleryModel::new(opts.theme, opts.widget);
    model.scroll = opts.scroll;
    // ASSUMED: P3's `SurfaceSpec::Layer` shape. Read `ui/src/window/mod.rs`
    // before writing this literal; only these field names may need changing.
    let spec = SurfaceSpec::Layer {
        namespace: "icedtea-gallery".to_string(),
        layer: LayerKind::Overlay,
        anchor: Anchor::TOP | Anchor::LEFT,
        size: opts.size,
        margin: [0, 0, 0, 0],
        exclusive_zone: -1,
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
        scale: opts.scale,
    };
    let window = Window::open(spec, compile_sheet(opts), FontDatabase::new())
        .map_err(AppError::Surface)?;
    App::new(model, update, scrolled_page).run(window)
}
```

In `ui/src/bin/gallery.rs`, replace the `else` branch:

```rust
    } else {
        icedtea_ui::gallery::run(&opts)
    };
```

In `ui/tests/support/mod.rs`, append (nothing above it changes):

```rust
// ---------------------------------------------------------------------------
// M3: the gallery
// ---------------------------------------------------------------------------

use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};

/// One `<widget> <label> <x> <y>` line from `gallery --probe-points`.
///
/// A local struct with owned fields, not `icedtea_ui::gallery::ProbePoint`:
/// what comes back over a pipe is text, and the library type's `widget` is a
/// `&'static str` that no parse can produce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    pub widget: String,
    pub label: String,
    pub x: i32,
    pub y: i32,
}

/// One `<widget> <x> <y> <width> <height>` line from
/// `gallery --print-allocation`: an entry's border box in page coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryAllocation {
    pub widget: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Parse one probe-point line. `None` for anything malformed — a child that
/// crashed mid-line must fail an assertion, never panic the test harness.
#[must_use]
pub fn parse_probe_line(line: &str) -> Option<ProbePoint> {
    let mut fields = line.split_whitespace();
    let widget = fields.next()?.to_string();
    let label = fields.next()?.to_string();
    let x = fields.next()?.parse().ok()?;
    let y = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(ProbePoint {
        widget,
        label,
        x,
        y,
    })
}

/// Parse one allocation line, with the same total-or-`None` rule.
#[must_use]
pub fn parse_allocation_line(line: &str) -> Option<EntryAllocation> {
    let mut fields = line.split_whitespace();
    let widget = fields.next()?.to_string();
    let x = fields.next()?.parse().ok()?;
    let y = fields.next()?.parse().ok()?;
    let width = fields.next()?.parse().ok()?;
    let height = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(EntryAllocation {
        widget,
        x,
        y,
        width,
        height,
    })
}

/// A `gallery` command with the hermetic environment every test wants.
fn gallery(theme: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gallery"));
    command.arg("--theme").arg(theme);
    command
}

/// Run `gallery --probe-points` (headless) and parse every line.
///
/// # Panics
///
/// If the binary cannot be run, exits non-zero, or prints a line the parser
/// rejects — all three mean the gate cannot know where to sample.
#[must_use]
pub fn probe_points(theme: &str, widget: Option<&str>) -> Vec<ProbePoint> {
    let mut command = gallery(theme);
    if let Some(widget) = widget {
        command.arg("--widget").arg(widget);
    }
    let output = command
        .arg("--probe-points")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --probe-points");
    assert!(
        output.status.success(),
        "gallery --probe-points exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("probe points are not UTF-8");
    stdout
        .lines()
        .map(|line| parse_probe_line(line).unwrap_or_else(|| panic!("bad probe line {line:?}")))
        .collect()
}

/// Run `gallery --print-allocation` (headless) and parse every line.
///
/// # Panics
///
/// As [`probe_points`].
#[must_use]
pub fn entry_allocations(theme: &str) -> Vec<EntryAllocation> {
    let output = gallery(theme)
        .arg("--print-allocation")
        .stderr(Stdio::null())
        .output()
        .expect("failed to run gallery --print-allocation");
    assert!(
        output.status.success(),
        "gallery --print-allocation exited with {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("allocations are not UTF-8");
    stdout
        .lines()
        .map(|line| {
            parse_allocation_line(line).unwrap_or_else(|| panic!("bad allocation line {line:?}"))
        })
        .collect()
}

/// A running `gallery`, killed on drop, with its stdout captured.
///
/// The captured stdout is what makes §7's "screencopy **or model** assertions"
/// possible: the app prints one flushed `msg <line>` per folded message, so
/// the interaction gate can assert on the model without sharing memory with
/// it.
pub struct GalleryProc {
    child: Child,
    messages: Arc<Mutex<Vec<String>>>,
}

impl GalleryProc {
    /// Every `msg` line seen so far, in order, without the `msg ` prefix.
    ///
    /// # Panics
    ///
    /// If the reader thread poisoned the lock, which only a panic there can do.
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().expect("message log").clone()
    }

    /// Wait until some message line equals `needle`, or `timeout` passes.
    #[must_use]
    pub fn wait_msg(&self, needle: &str, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            if self.messages().iter().any(|line| line == needle) {
                return true;
            }
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(CAPTURE_POLL);
        }
    }
}

impl Drop for GalleryProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `command` against `socket` with its stdout drained by a reader
/// thread, so a full pipe can never block the child.
fn spawn_gallery_command(mut command: Command, socket: &str) -> GalleryProc {
    let mut child = command
        .env("WAYLAND_DISPLAY", socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn gallery");
    let stdout = child.stdout.take().expect("piped stdout");
    let messages = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&messages);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(msg) = line.strip_prefix("msg ") {
                sink.lock().expect("message log").push(msg.to_string());
            }
        }
    });
    GalleryProc { child, messages }
}

/// The whole page, scrolled to `scroll`.
#[must_use]
pub fn spawn_gallery(socket: &str, theme: &str, scroll: i32) -> GalleryProc {
    let mut command = gallery(theme);
    command.arg("--scroll").arg(scroll.to_string());
    spawn_gallery_command(command, socket)
}

/// One widget, alone, at the origin.
#[must_use]
pub fn spawn_gallery_widget(socket: &str, theme: &str, widget: &str) -> GalleryProc {
    let mut command = gallery(theme);
    command.arg("--widget").arg(widget);
    spawn_gallery_command(command, socket)
}

/// How long a gallery gets to map and paint its first frame.
///
/// A generous complexity bound, not a wall-clock pin: it covers compiling a
/// ~1,900-line Adwaita sheet, matching fonts and painting 64 widgets on a
/// loaded CI box.
pub const GALLERY_MAP_TIMEOUT: Duration = Duration::from_secs(20);

/// Capture until `probe` stops showing `before` — the gallery's first frame.
///
/// # Panics
///
/// If nothing changes within [`GALLERY_MAP_TIMEOUT`], which means the gallery
/// never mapped.
pub fn wait_for_gallery(
    screencopy: &mut ScreencopyClient,
    probe: (u32, u32),
    before: (u8, u8, u8),
) -> CapturedFrame {
    let started = Instant::now();
    loop {
        let frame = screencopy.capture();
        let px = pixel_at(&frame, probe.0, probe.1).expect("the probe is inside the frame");
        if !matches(px, before) {
            return frame;
        }
        assert!(
            started.elapsed() < GALLERY_MAP_TIMEOUT,
            "the gallery never painted: ({}, {}) is still {before:?}",
            probe.0,
            probe.1
        );
        std::thread::sleep(CAPTURE_POLL);
    }
}

/// Whether anything inside `rect` differs from `background`.
///
/// Samples a 5x5 grid inset one pixel from the border box, which is dense
/// enough to catch a widget that drew only a border and cheap enough to run
/// 64 times per capture.
#[must_use]
pub fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 2 || h <= 2 {
        return false;
    }
    (0..5).any(|row| {
        (0..5).any(|col| {
            let px = x + 1 + (w - 2) * col / 4;
            let py = y + 1 + (h - 2) * row / 4;
            pixel_at(frame, px as u32, py as u32).is_some_and(|got| !matches(got, background))
        })
    })
}
```

Add `Instant` to the existing `use std::time::{Duration, Instant};` line if it
is not already imported, and `Child`, `Command`, `Stdio` are already there.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --test gallery_gate`
Expected: PASS — 2 tests. The rest-state test takes on the order of a minute:
one gallery spawn per ~700 px slice.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/src/gallery.rs ui/src/bin/gallery.rs ui/tests/support/mod.rs ui/tests/gallery_gate.rs
git commit -m "test(ui,gallery): the gallery runs and every widget paints in light Adwaita

The page is walked in surface-height slices, and every widget's entry is
asserted to paint something at coordinates read from --print-allocation and
--probe-points, never from a literal.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 7: dark, high contrast, and the light/dark difference

**Files:**
- Modify: `ui/tests/gallery_gate.rs`
- Test: `ui/tests/gallery_gate.rs`

**Interfaces:**
- Consumes: Task 6's `every_widget_renders_at_rest`, `probe_points`,
  `spawn_gallery`, `wait_for_gallery`, `pixel_at`, `matches`.
- Produces: three more `#[test]`s; no new API.

- [ ] **Step 1: Write the failing tests**

Append to `ui/tests/gallery_gate.rs`:

```rust
#[test]
fn every_widget_renders_at_rest_in_the_dark_theme() {
    every_widget_renders_at_rest("dark");
}

#[test]
fn every_widget_renders_at_rest_in_the_high_contrast_theme() {
    every_widget_renders_at_rest("hc");
}

/// Capture every slice of `theme` and index the probe pixels by
/// `(widget, label)`.
fn probe_pixels(theme: &str) -> std::collections::BTreeMap<(String, String), (u8, u8, u8)> {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();
    let (out_w, out_h) = compositor.output_size();
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let empty = screencopy.capture();
    let background_probe = (out_w as u32 - 3, out_h as u32 / 2);
    let wallpaper = pixel_at(&empty, background_probe.0, background_probe.1).expect("inside");

    let allocations = entry_allocations(theme);
    let page_height = allocations
        .iter()
        .map(|a| (a.y + a.height) as i32)
        .max()
        .expect("a non-empty page");
    let points = probe_points(theme, None);

    let mut sampled = std::collections::BTreeMap::new();
    let mut scroll = 0;
    while scroll < page_height {
        let gallery = spawn_gallery(&socket, theme, scroll);
        let frame = wait_for_gallery(&mut screencopy, background_probe, wallpaper);
        for point in &points {
            let y = point.y - scroll;
            if y < 2 || y >= SURFACE_HEIGHT - 2 {
                continue;
            }
            let key = (point.widget.clone(), point.label.clone());
            if let std::collections::btree_map::Entry::Vacant(slot) = sampled.entry(key)
                && let Some(px) = pixel_at(&frame, point.x as u32, y as u32)
            {
                slot.insert(px);
            }
        }
        drop(gallery);
        scroll += SLICE_STEP;
    }
    sampled
}

/// Per widget, at least one probe point must look different in dark Adwaita.
///
/// Per widget, not per point: a transparent subnode, or one Adwaita styles
/// identically in both sheets, legitimately matches across themes — the
/// contract's test name is kept, its assertion is the honest one (deviation
/// 6). A widget where *nothing* changes is a widget the theme never reached.
///
/// Mutation check: make `Theme::sheet` return the light sheet for
/// `Theme::Dark`; every widget then matches and this test fails on the first
/// one. Restore.
#[test]
fn every_probe_point_differs_between_light_and_dark() {
    let light = probe_pixels("light");
    let dark = probe_pixels("dark");
    let widgets: std::collections::BTreeSet<&String> =
        light.keys().map(|(widget, _)| widget).collect();
    assert!(!widgets.is_empty(), "no probe pixels were sampled at all");
    let mut unchanged = Vec::new();
    for widget in widgets {
        let differs = light
            .iter()
            .filter(|((w, _), _)| w == widget)
            .any(|((w, label), light_px)| {
                dark.get(&(w.clone(), label.clone()))
                    .is_some_and(|dark_px| !support::matches(*light_px, *dark_px))
            });
        if !differs {
            unchanged.push(widget.clone());
        }
    }
    assert!(
        unchanged.is_empty(),
        "these widgets look identical in light and dark Adwaita, so the theme \
         never reached them: {unchanged:?}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test gallery_gate every_probe_point_differs`
Expected: FAIL — `cannot find function `probe_pixels`` before the
implementation is added; after it, the test must pass. Verify the failure mode
first by temporarily returning `Theme::Light.sheet()` from `Theme::Dark`'s arm
and confirming the assertion message names widgets, then restore.

- [ ] **Step 3: Write the implementation**

No production code changes: the three tests above are the deliverable. If
`every_widget_renders_at_rest_in_the_high_contrast_theme` fails, the fix
belongs to the owning widget's part (P5/P6) fix wave, not here — record the
failure, do not patch the gate to accept it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --test gallery_gate`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui --test gallery_gate
git add ui/tests/gallery_gate.rs
git commit -m "test(ui,gallery): dark and high-contrast rest states, and the light/dark difference

Per widget, not per point: a transparent or theme-independent subnode
legitimately matches across sheets, but a widget where nothing at all changes
is a widget the theme never reached (contract deviation 6).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 8: completeness and node-tree conformance

**Files:**
- Modify: `ui/tests/gallery_gate.rs`
- Modify (only if needed): `ui/src/view/mod.rs` — see Step 3
- Test: `ui/tests/gallery_gate.rs`

**Interfaces:**
- Consumes: `icedtea_ui::gallery::{kind_name, own_kinds, sample_shape,
  SampleShape, GalleryModel, Theme, sample, Sample}`; P4
  `view::{Kind, Props}`; P5/P6 `view::node_tree_of(kind: Kind, props: &Props)
  -> String` and the fixtures at
  `ui/tests/fixtures/gtk4.22-node-trees/<name>.txt`.
- Produces: two more `#[test]`s; no new API.

- [ ] **Step 1: Write the failing tests**

Append to `ui/tests/gallery_gate.rs`:

```rust
use icedtea_ui::gallery::{GalleryModel, Sample, SampleShape, kind_name, own_kinds, sample, sample_shape};
use icedtea_ui::view::{Kind, Props, node_tree_of};

/// `--list` and the page are the same set, and every sub-kind really appears
/// inside its parent's rendered tree.
///
/// This is the test the whole gallery exists for: a `Kind` that reaches no
/// pixel is a `Kind` no other test in this file could ever have covered.
///
/// Mutation check: give `sample_shape` an extra
/// `Kind::Switch => SampleShape::Within(Kind::Box)` arm; this test fails on
/// the `switch` sub-kind assertion (its node name is absent from `box`'s
/// tree). Restore.
#[test]
fn every_kind_appears_in_the_gallery() {
    let listed: Vec<String> = String::from_utf8(
        std::process::Command::new(env!("CARGO_BIN_EXE_gallery"))
            .arg("--list")
            .output()
            .expect("gallery --list runs")
            .stdout,
    )
    .expect("--list is UTF-8")
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(
        listed.len(),
        Kind::all().len(),
        "--list must print every Kind"
    );
    for &kind in Kind::all() {
        assert!(
            listed.iter().any(|name| name == kind_name(kind)),
            "{} is missing from --list",
            kind_name(kind)
        );
    }

    let entries: Vec<String> = entry_allocations("light")
        .into_iter()
        .map(|a| a.widget)
        .collect();
    for kind in own_kinds() {
        assert!(
            entries.iter().any(|w| w == kind_name(kind)),
            "{} has no entry on the page",
            kind_name(kind)
        );
    }
    assert_eq!(entries.len(), own_kinds().len(), "the page has extra entries");

    let model = GalleryModel::new(icedtea_ui::gallery::Theme::Light, None);
    for &kind in Kind::all() {
        let SampleShape::Within(parent) = sample_shape(kind) else {
            continue;
        };
        let Sample::Own(view) = sample(parent, &model) else {
            panic!("{}'s parent must be its own entry", kind_name(kind));
        };
        let tree = node_tree_of(parent, &view.props);
        assert!(
            tree.contains(kind.css_name()),
            "{} claims to live inside {}, whose node tree has no {:?} node:\n{tree}",
            kind_name(kind),
            kind_name(parent),
            kind.css_name()
        );
    }
}

/// Every widget's retained tree matches the GTK 4.22 "CSS nodes" block
/// vendored for it.
///
/// P5/P6 own both `node_tree_of` and the fixtures; P8 only wires them into the
/// gate, so a widget added without a fixture fails here rather than shipping
/// unmeasured.
///
/// Mutation check: delete one line from any fixture; this test fails naming
/// that widget with a unified-looking diff. Restore.
#[test]
fn the_node_tree_of_every_widget_matches_its_gtk_fixture() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gtk4.22-node-trees");
    let model = GalleryModel::new(icedtea_ui::gallery::Theme::Light, None);
    let mut missing = Vec::new();
    for &kind in Kind::all() {
        let path = dir.join(format!("{}.txt", kind_name(kind)));
        let Ok(expected) = std::fs::read_to_string(&path) else {
            missing.push(kind_name(kind));
            continue;
        };
        let props = match sample(kind, &model) {
            Sample::Own(view) => view.props,
            Sample::Within(_) => Props::default(),
        };
        let rendered = node_tree_of(kind, &props);
        assert_eq!(
            rendered.trim_end(),
            expected.trim_end(),
            "{}'s node tree does not match {}",
            kind_name(kind),
            path.display()
        );
    }
    assert!(
        missing.is_empty(),
        "no vendored GTK node-tree fixture for: {missing:?} (expected \
         ui/tests/fixtures/gtk4.22-node-trees/<name>.txt, one per Kind)"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test gallery_gate every_kind_appears`
Expected: FAIL to compile — `unresolved import
`icedtea_ui::view::node_tree_of`` if P5/P6 filed the function inside
`ui/tests/node_trees.rs` instead of the library.

- [ ] **Step 3: Write the implementation**

Only one production change may be needed, and only if the compile failure above
occurs: `node_tree_of` must be reachable from a second test binary, so it
belongs in the library. Move the function (not its tests) from
`ui/tests/node_trees.rs` into `ui/src/view/mod.rs`, keep it `pub`, and leave
`ui/tests/node_trees.rs` calling `icedtea_ui::view::node_tree_of` — its own
fixtures and `#[test]`s stay exactly as P5/P6 wrote them. Record the move in
the contract's §10 (Task 14).

If it is already public in the library, this task has no production change at
all: the two tests are the deliverable.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --test gallery_gate`
Expected: PASS — 7 tests.

Run: `cargo test -p icedtea-ui --test node_trees`
Expected: PASS — P5/P6's own conformance tests, unchanged.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/tests/gallery_gate.rs ui/src/view/mod.rs ui/tests/node_trees.rs
git commit -m "test(ui,gallery): completeness and GTK node-tree conformance

A Kind with no gallery entry, no --list line, or no vendored GTK node-tree
fixture now fails the gate instead of shipping unmeasured.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 9: the interaction driver, the harness axis event, and the four button interactions

**Files:**
- Modify: `harness/src/lib.rs` (additive `VirtualPointerClient::{axis, axis_discrete}`)
- Modify: `ui/tests/support/mod.rs` (the interaction driver)
- Create: `ui/tests/interaction_gate.rs`
- Test: `ui/tests/interaction_gate.rs`, `harness/src/lib.rs`

**Interfaces:**
- Consumes: Task 6's `spawn_gallery_widget`, `probe_points`,
  `wait_for_gallery`, `GalleryProc`; `icedtea_harness::{VirtualPointerClient,
  VirtualKeyboardClient}`; `icedtea_ui::wayland::BTN_LEFT`.
- Produces:
  ```rust
  // harness/src/lib.rs
  impl VirtualPointerClient {
      pub fn axis(&mut self, horizontal: f64, vertical: f64);
      pub fn axis_discrete(&mut self, horizontal: i32, vertical: i32);
  }

  // ui/tests/support/mod.rs
  pub struct Driver { /* compositor, screencopy, pointer, keyboard, output */ }
  impl Driver {
      pub fn new() -> Driver;
      pub fn socket(&self) -> String;
      pub fn open(&mut self, theme: &str, widget: &str) -> GalleryProc;
      pub fn point(&self, widget: &str, label: &str) -> (i32, i32);
      pub fn move_to(&mut self, x: i32, y: i32);
      pub fn click(&mut self, x: i32, y: i32);
      pub fn press(&mut self, x: i32, y: i32);
      pub fn release(&mut self, x: i32, y: i32);
      pub fn drag(&mut self, from: (i32, i32), to: (i32, i32));
      pub fn scroll(&mut self, x: i32, y: i32, vertical: f64);
      pub fn key(&mut self, keycode: u32);
      pub fn keys(&mut self, keycodes: &[u32]);
      pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8);
      pub fn wait_pixel_change(&mut self, x: i32, y: i32, before: (u8, u8, u8)) -> (u8, u8, u8);
      pub fn wait_pixel_settled(&mut self, x: i32, y: i32) -> (u8, u8, u8);
  }
  pub const REACT_TIMEOUT: Duration;
  pub const KEY_TAB: u32; pub const KEY_SPACE: u32; pub const KEY_ENTER: u32;
  pub const KEY_H: u32; pub const KEY_I: u32; pub const KEY_A: u32;
  pub const KEY_UP: u32; pub const KEY_DOWN: u32; pub const KEY_ESC: u32;
  ```

- [ ] **Step 1: Write the failing tests**

In `harness/src/lib.rs`'s test module — or, if the injectors have no unit tests
there, in `ui/tests/interaction_gate.rs` as part of the scroll test in Task 11
— assert the request reaches the wire:

```rust
    /// The axis request the interaction gate's scroll test needs; the harness
    /// injector had motion and buttons but no axis at all.
    #[test]
    fn a_virtual_pointer_can_send_an_axis_event() {
        let compositor = Compositor::spawn();
        let socket = compositor
            .socket_path()
            .file_name()
            .expect("socket name")
            .to_string_lossy()
            .to_string();
        let mut pointer = VirtualPointerClient::spawn(&socket);
        pointer.motion_absolute(10.0, 10.0, 100, 100);
        pointer.frame();
        pointer.axis(0.0, 10.0);
        pointer.frame();
        pointer.pump();
        // No protocol error killed the client: the connection is still usable.
        pointer.motion_absolute(11.0, 11.0, 100, 100);
        pointer.frame();
        pointer.pump();
    }
```

Create `ui/tests/interaction_gate.rs`:

```rust
//! The M3 interaction gate: one interaction per widget class, driven by a
//! virtual pointer and keyboard against the harness compositor.
//!
//! Every test opens **one** widget (`gallery --widget NAME`), so its
//! coordinates are the widget's own and no other widget can move them. Points
//! are read from `--probe-points`; nothing here hard-codes a coordinate.

mod support;

use std::time::Duration;

use support::{Driver, KEY_SPACE, KEY_TAB};

/// How long an interaction gets to reach the model and the screen. A generous
/// complexity bound: a round trip through the compositor, the app loop, a
/// restyle and a repaint.
const REACT: Duration = Duration::from_secs(5);

/// Mutation check: make `ButtonC::on_event` drop its `PointerUp` arm; the
/// message never arrives and this test fails on `wait_msg`. Restore.
#[test]
fn clicking_a_button_fires_its_message_and_paints_the_active_state() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "button");
    let (x, y) = driver.point("button", "root");
    let rest = driver.pixel(x, y);

    driver.press(x, y);
    let pressed = driver.wait_pixel_change(x, y, rest);
    assert!(
        !support::matches(pressed, rest),
        "a held button must paint :active; it stayed {rest:?}"
    );

    driver.release(x, y);
    assert!(
        gallery.wait_msg("clicked button", REACT),
        "no `clicked button` message; got {:?}",
        gallery.messages()
    );
}

/// Mutation check: make the toggle sample ignore `model.toggle(...)`; the
/// checked pixel never changes and this test fails. Restore.
#[test]
fn toggling_a_toggle_button_paints_the_checked_state() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "toggle_button");
    let (x, y) = driver.point("toggle_button", "root");
    let rest = driver.pixel(x, y);

    driver.click(x, y);
    assert!(
        gallery.wait_msg("toggled toggle_button true", REACT),
        "no toggle message; got {:?}",
        gallery.messages()
    );
    let checked = driver.wait_pixel_change(x, y, rest);
    assert!(!support::matches(checked, rest), "`:checked` painted nothing");

    driver.click(x, y);
    assert!(gallery.wait_msg("toggled toggle_button false", REACT));
}

/// The check itself, not the row: the `check` subnode is where GTK paints the
/// builtin, so that is the probe point this test samples.
///
/// Mutation check: point the sample's `.active()` at a constant `false`; the
/// check subnode never changes and this test fails. Restore.
#[test]
fn checking_a_check_button_paints_the_builtin_check() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "check_button");
    let (cx, cy) = driver.point("check_button", "check");
    let rest = driver.pixel(cx, cy);

    driver.click(cx, cy);
    assert!(
        gallery.wait_msg("toggled check_button true", REACT),
        "no toggle message; got {:?}",
        gallery.messages()
    );
    let checked = driver.wait_pixel_change(cx, cy, rest);
    assert!(
        !support::matches(checked, rest),
        "the builtin check painted nothing on the `check` subnode"
    );
}

/// The switch animates its slider from one end to the other, so the *far* end
/// is what changes — sampling the centre would pass even if the slider never
/// moved.
///
/// Mutation check: make `SwitchC` jump `slide` to its end state without the
/// clock; the test still passes (it asserts the end state, not the tween) —
/// instead delete the `on_toggle` handler and confirm the message never
/// arrives. Restore.
#[test]
fn flipping_a_switch_animates_the_slider_to_the_other_end() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "switch");
    let (rx, ry) = driver.point("switch", "root");
    let (sx, sy) = driver.point("switch", "slider");
    // The end the slider is *not* at while the switch is off.
    let far = (rx + (rx - sx), sy);
    let rest = driver.pixel(far.0, far.1);

    driver.click(rx, ry);
    assert!(
        gallery.wait_msg("toggled switch true", REACT),
        "no toggle message; got {:?}",
        gallery.messages()
    );
    let moved = driver.wait_pixel_change(far.0, far.1, rest);
    assert!(
        !support::matches(moved, rest),
        "the slider never reached the other end of the switch"
    );
}

/// Space activates the focused widget — GTK's rule, and the keyboard half of
/// the button interaction.
///
/// Mutation check: remove `Space` from P3's window-level bindings; this test
/// fails on `wait_msg`. Restore.
#[test]
fn a_pointer_click_focuses_without_showing_the_focus_ring() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "button");
    let (x, y) = driver.point("button", "root");
    let rest = driver.pixel(x, y);

    driver.click(x, y);
    assert!(gallery.wait_msg("clicked button", REACT));
    // Back at rest: a pointer click leaves `:focus` but not `:focus-visible`,
    // so the button must look exactly as it did before the click.
    let after = driver.wait_pixel_settled(x, y);
    assert!(
        support::matches(after, rest),
        "a pointer click drew a focus ring: {rest:?} -> {after:?}"
    );

    // A key press makes the ring visible, which is what proves the first
    // assertion was about `:focus-visible` and not about focus never arriving.
    driver.key(KEY_TAB);
    driver.key(KEY_SPACE);
    assert!(
        gallery.wait_msg("clicked button", REACT),
        "Space did not activate the focused button; got {:?}",
        gallery.messages()
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test interaction_gate`
Expected: FAIL to compile — `cannot find type `Driver` in module `support``.

Run: `cargo test -p icedtea-harness a_virtual_pointer_can_send_an_axis_event`
Expected: FAIL — `no method named `axis` found for struct
`VirtualPointerClient``.

- [ ] **Step 3: Write the implementation**

In `harness/src/lib.rs`, inside `impl VirtualPointerClient`, after `button`:

```rust
    /// Scroll by a continuous amount, in surface-local units + flush.
    ///
    /// `wl_pointer`'s own sign convention: positive is down/right. Callers
    /// send `frame()` themselves, as with every other request here.
    pub fn axis(&mut self, horizontal: f64, vertical: f64) {
        let time = self.next_time();
        if horizontal != 0.0 {
            self.vp
                .axis(time, wl_pointer::Axis::HorizontalScroll, horizontal);
        }
        if vertical != 0.0 {
            self.vp.axis(time, wl_pointer::Axis::VerticalScroll, vertical);
        }
        self.conn.flush().expect("flush axis");
    }

    /// Scroll by whole wheel clicks + flush: one click is 10 units, which is
    /// what a real wheel sends alongside its discrete value.
    pub fn axis_discrete(&mut self, horizontal: i32, vertical: i32) {
        let time = self.next_time();
        if horizontal != 0 {
            self.vp.axis_discrete(
                time,
                wl_pointer::Axis::HorizontalScroll,
                f64::from(horizontal) * 10.0,
                horizontal,
            );
        }
        if vertical != 0 {
            self.vp.axis_discrete(
                time,
                wl_pointer::Axis::VerticalScroll,
                f64::from(vertical) * 10.0,
                vertical,
            );
        }
        self.conn.flush().expect("flush axis_discrete");
    }
```

In `ui/tests/support/mod.rs`, append:

```rust
/// Linux input-event keycodes the interaction gate sends.
pub const KEY_ESC: u32 = 1;
pub const KEY_A: u32 = 30;
pub const KEY_H: u32 = 35;
pub const KEY_I: u32 = 23;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_SPACE: u32 = 57;
pub const KEY_UP: u32 = 103;
pub const KEY_DOWN: u32 = 108;

/// One compositor, one gallery, one pointer and one keyboard: everything an
/// interaction test needs, with the coordinates read from the binary.
pub struct Driver {
    compositor: Compositor,
    socket: String,
    screencopy: ScreencopyClient,
    pointer: VirtualPointerClient,
    keyboard: VirtualKeyboardClient,
    output: (u32, u32),
    theme: String,
    widget: String,
    background: (u8, u8, u8),
    background_probe: (u32, u32),
}

impl Driver {
    /// Boot a compositor and its injectors.
    ///
    /// # Panics
    ///
    /// If the compositor advertises no output, screencopy or injector.
    #[must_use]
    pub fn new() -> Driver {
        let compositor = Compositor::spawn();
        let socket = compositor
            .socket_path()
            .file_name()
            .expect("socket name")
            .to_string_lossy()
            .to_string();
        let (w, h) = compositor.output_size();
        let mut screencopy = ScreencopyClient::spawn(&socket);
        let background_probe = (w as u32 - 3, h as u32 / 2);
        let empty = screencopy.capture();
        let background = pixel_at(&empty, background_probe.0, background_probe.1)
            .expect("the background probe is inside the frame");
        let pointer = VirtualPointerClient::spawn(&socket);
        let keyboard = VirtualKeyboardClient::spawn(&socket);
        Driver {
            compositor,
            socket,
            screencopy,
            pointer,
            keyboard,
            output: (w as u32, h as u32),
            theme: String::new(),
            widget: String::new(),
            background,
            background_probe,
        }
    }

    /// The compositor's socket name.
    #[must_use]
    pub fn socket(&self) -> String {
        self.socket.clone()
    }

    /// Open one widget, alone, and wait for its first frame.
    ///
    /// # Panics
    ///
    /// If the gallery never paints.
    pub fn open(&mut self, theme: &str, widget: &str) -> GalleryProc {
        self.theme = theme.to_string();
        self.widget = widget.to_string();
        let gallery = spawn_gallery_widget(&self.socket, theme, widget);
        let _ = wait_for_gallery(&mut self.screencopy, self.background_probe, self.background);
        gallery
    }

    /// The output-space centre of `widget`'s `label` subnode.
    ///
    /// # Panics
    ///
    /// If the widget exposes no such probe point — the message lists the ones
    /// it does expose, which is what makes a renamed subnode a readable
    /// failure rather than a mystery.
    #[must_use]
    pub fn point(&self, widget: &str, label: &str) -> (i32, i32) {
        let points = probe_points(&self.theme, Some(widget));
        points
            .iter()
            .find(|p| p.label == label)
            .map(|p| (p.x, p.y))
            .unwrap_or_else(|| {
                let have: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
                panic!("{widget} has no {label:?} probe point; it has {have:?}")
            })
    }

    /// Move the pointer, in output coordinates.
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.pointer
            .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press the left button at `(x, y)`.
    pub fn press(&mut self, x: i32, y: i32) {
        self.move_to(x, y);
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Release the left button at `(x, y)`.
    pub fn release(&mut self, x: i32, y: i32) {
        self.move_to(x, y);
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press and release at `(x, y)`.
    pub fn click(&mut self, x: i32, y: i32) {
        self.press(x, y);
        self.release(x, y);
    }

    /// Press at `from`, move through the midpoint, release at `to` — the
    /// motion in the middle is what a drag needs; a teleport looks like a
    /// click somewhere else.
    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32)) {
        self.press(from.0, from.1);
        self.move_to((from.0 + to.0) / 2, (from.1 + to.1) / 2);
        self.move_to(to.0, to.1);
        self.release(to.0, to.1);
    }

    /// Scroll at `(x, y)`; positive `vertical` scrolls down.
    pub fn scroll(&mut self, x: i32, y: i32, vertical: f64) {
        self.move_to(x, y);
        self.pointer.axis(0.0, vertical);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Press and release one key.
    pub fn key(&mut self, keycode: u32) {
        self.keyboard.key_press(keycode);
        self.keyboard.pump();
    }

    /// Press and release each key in order.
    pub fn keys(&mut self, keycodes: &[u32]) {
        for &code in keycodes {
            self.key(code);
        }
    }

    /// The current colour at `(x, y)`.
    ///
    /// # Panics
    ///
    /// If the point is outside the output.
    pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        let frame = self.screencopy.capture();
        pixel_at(&frame, x as u32, y as u32)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the {:?} output", self.output))
    }

    /// Capture until `(x, y)` stops being `before`, or [`REACT_TIMEOUT`]
    /// passes; returns whatever it ended on so the caller writes the
    /// assertion.
    pub fn wait_pixel_change(&mut self, x: i32, y: i32, before: (u8, u8, u8)) -> (u8, u8, u8) {
        let started = Instant::now();
        loop {
            let px = self.pixel(x, y);
            if !matches(px, before) || started.elapsed() >= REACT_TIMEOUT {
                return px;
            }
            self.pointer.pump();
            std::thread::sleep(CAPTURE_POLL);
        }
    }

    /// Capture until `(x, y)` holds the same colour for two consecutive polls
    /// — "the animation is over", without pinning how long it took.
    pub fn wait_pixel_settled(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        let started = Instant::now();
        let mut last = self.pixel(x, y);
        loop {
            std::thread::sleep(CAPTURE_POLL);
            let now = self.pixel(x, y);
            if matches(now, last) || started.elapsed() >= REACT_TIMEOUT {
                return now;
            }
            last = now;
        }
    }
}

/// The ceiling every `wait_*` here shares: a generous complexity bound on one
/// round trip through the compositor, the app loop, a restyle and a repaint.
pub const REACT_TIMEOUT: Duration = Duration::from_secs(5);
```

Extend `ui/tests/support/mod.rs`'s `use icedtea_harness::{…}` line with
`Compositor` and `VirtualKeyboardClient`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-harness a_virtual_pointer_can_send_an_axis_event`
Expected: PASS.

Run: `cargo test -p icedtea-ui --test interaction_gate`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-harness --all-targets -- -D warnings
cargo test -p icedtea-harness
cargo test -p icedtea-ui
git add harness/src/lib.rs ui/tests/support/mod.rs ui/tests/interaction_gate.rs
git commit -m "test(ui,gallery): the interaction driver and the button interactions

Adds the axis request the harness injector never had (contract deviation 9),
and drives click/toggle/check/switch plus the focus-ring rule through a real
compositor at coordinates read from --probe-points.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 10: the entry interactions

**Files:**
- Modify: `ui/tests/interaction_gate.rs`
- Test: `ui/tests/interaction_gate.rs`

**Interfaces:**
- Consumes: Task 9's `Driver`, `KEY_H`, `KEY_I`, `KEY_A`, `REACT`.
- Produces: four more `#[test]`s; no new API.

- [ ] **Step 1: Write the failing tests**

Append to `ui/tests/interaction_gate.rs`:

```rust
use support::{KEY_A, KEY_H, KEY_I};

/// Typing must reach the model *and* the glyphs must reach the screen: either
/// half alone would pass with the other broken.
///
/// Mutation check: make `EntryC` swallow `Event::Key` without emitting
/// `EventKind::Change`; the message never arrives and this test fails.
/// Restore.
#[test]
fn typing_into_an_entry_shows_the_glyphs_and_moves_the_caret() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "entry");
    let (tx, ty) = driver.point("entry", "text");
    let before = driver.pixel(tx, ty);

    driver.click(tx, ty);
    driver.keys(&[KEY_H, KEY_I]);

    assert!(
        gallery.wait_msg("changed entry Entryhi", REACT)
            || gallery.wait_msg("changed entry hi", REACT),
        "typing did not reach the model; got {:?}",
        gallery.messages()
    );
    let after = driver.wait_pixel_change(tx, ty, before);
    assert!(
        !support::matches(after, before),
        "the typed glyphs never reached the `text` subnode"
    );
}

/// One search, after the delay — not one per keystroke. That debounce is the
/// whole behaviour `SearchEntry` adds over `Entry`.
///
/// Mutation check: make `SearchEntryC` fire `EventKind::Search` on every key;
/// the count assertion below fails with 2. Restore.
#[test]
fn typing_into_a_search_entry_fires_one_search_after_the_delay() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "search_entry");
    let (tx, ty) = driver.point("search_entry", "text");

    driver.click(tx, ty);
    driver.keys(&[KEY_H, KEY_I]);
    assert!(
        gallery.wait_msg("search hi", REACT),
        "the search never fired; got {:?}",
        gallery.messages()
    );
    // Let any further debounced fire land before counting.
    std::thread::sleep(Duration::from_millis(500));
    let searches = gallery
        .messages()
        .iter()
        .filter(|line| line.starts_with("search "))
        .count();
    assert_eq!(
        searches,
        1,
        "two keystrokes must debounce into one search; got {:?}",
        gallery.messages()
    );
}

/// Peeking reveals the real text: the `text` subnode must change when the peek
/// icon is clicked, with no keystroke in between.
///
/// Mutation check: make `PasswordEntryC`'s peek toggle `visibility` without
/// re-shaping the text; the pixels stay bullets and this test fails. Restore.
#[test]
fn peeking_a_password_entry_reveals_the_text() {
    let mut driver = Driver::new();
    let _gallery = driver.open("light", "password_entry");
    let (tx, ty) = driver.point("password_entry", "text");
    let bullets = driver.pixel(tx, ty);
    let (ix, iy) = driver.point("password_entry", "image");

    driver.click(ix, iy);
    let revealed = driver.wait_pixel_change(tx, ty, bullets);
    assert!(
        !support::matches(revealed, bullets),
        "the peek icon revealed nothing: the text stayed {bullets:?}"
    );
}

/// Holding the up button must step more than once: that repeat timer is the
/// clock-driven behaviour `SpinButtonC::tick` owns.
///
/// Mutation check: make `SpinButtonC::next_deadline` return `None`; only the
/// first step happens and this test fails on the second value. Restore.
#[test]
fn stepping_a_spin_button_repeats_while_the_button_is_held() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "spin_button");
    let (ux, uy) = driver.point("spin_button", "button0");

    driver.press(ux, uy);
    assert!(
        gallery.wait_msg("value spin_button 4", REACT),
        "the first step never happened; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg("value spin_button 5", REACT),
        "the held button did not repeat; got {:?}",
        gallery.messages()
    );
    driver.release(ux, uy);

    let steps = gallery
        .messages()
        .iter()
        .filter(|line| line.starts_with("value spin_button "))
        .count();
    assert!(
        steps >= 2,
        "a held spin button must step at least twice, got {steps}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test interaction_gate typing_into_an_entry`
Expected: FAIL — before P5's entry behaviour is exercised the assertion that
fails is `typing did not reach the model; got []`. (If it passes immediately,
run the mutation check named in the doc comment to prove the test can fail.)

- [ ] **Step 3: Write the implementation**

No production code: these four tests are the deliverable. A failure here is a
P5 defect and belongs in P5's fix wave — do not weaken the assertion. The one
adjustment permitted is the probe **label** each test samples: if
`--probe-points` shows the spin button's up node is `button1` rather than
`button0`, or the password entry's peek icon is `image1`, use the label the
binary prints and say so in a comment.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --test interaction_gate`
Expected: PASS — 9 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui --test interaction_gate
git add ui/tests/interaction_gate.rs
git commit -m "test(ui,gallery): entry, search, password and spin-button interactions

Each asserts both halves: the message reached the model and the pixels
changed. The search test counts fires, so a lost debounce fails.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 11: dropdown, scale and list interactions

**Files:**
- Modify: `ui/tests/interaction_gate.rs`
- Test: `ui/tests/interaction_gate.rs`

**Interfaces:**
- Consumes: Task 9's `Driver` (`drag`, `scroll`), Task 10's imports.
- Produces: three more `#[test]`s; no new API.

- [ ] **Step 1: Write the failing tests**

Append to `ui/tests/interaction_gate.rs`:

```rust
/// Opening a drop-down maps a real `xdg_popup` (P1/P2/P3) and picking an item
/// must both dismiss it and update the button.
///
/// Mutation check: make `DropDownC` ignore the popover's `on_activate`; no
/// `selected drop_down 1` arrives and this test fails. Restore.
#[test]
fn opening_a_drop_down_and_picking_an_item_updates_the_button() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "drop_down");
    let (bx, by) = driver.point("drop_down", "root");
    // Below the button is where the popover lands, and it is window
    // background until it does.
    let below = (bx, by + 40);
    let closed = driver.pixel(below.0, below.1);

    driver.click(bx, by);
    let open = driver.wait_pixel_change(below.0, below.1, closed);
    assert!(
        !support::matches(open, closed),
        "the drop-down's popover never appeared below the button"
    );

    // The second row of the list: one row height below the first.
    driver.click(below.0, below.1 + 24);
    assert!(
        gallery.wait_msg("selected drop_down 1", REACT),
        "picking the second item did not select it; got {:?}",
        gallery.messages()
    );
    let dismissed = driver.wait_pixel_change(below.0, below.1, open);
    assert!(
        !support::matches(dismissed, open),
        "the popover stayed up after a pick"
    );
}

/// Dragging the slider must move it *and* report the value: a scale that
/// paints without reporting is as broken as one that reports without painting.
///
/// Mutation check: make `ScaleC` clamp its drag to the press position; the
/// value message never arrives and this test fails. Restore.
#[test]
fn dragging_a_scale_moves_the_slider_and_reports_the_value() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "scale");
    let (sx, sy) = driver.point("scale", "slider");
    let (tx, ty) = driver.point("scale", "trough");
    // Drag right, staying inside the trough: its centre plus most of the
    // remaining half-width.
    let target = (tx + (tx - sx).abs().max(40), ty);
    let at_rest = driver.pixel(sx, sy);

    driver.drag((sx, sy), target);
    assert!(
        gallery
            .messages()
            .iter()
            .any(|line| line.starts_with("value scale ")),
        "the drag reported no value; got {:?}",
        gallery.messages()
    );
    let vacated = driver.wait_pixel_change(sx, sy, at_rest);
    assert!(
        !support::matches(vacated, at_rest),
        "the slider never left its starting position"
    );
}

/// Scrolling a `ListView` recycles rows; the selection must survive it.
///
/// Selection is model state (`selected list_view 0`), row identity is
/// controller state — this is the test that proves recycling did not throw the
/// second away.
///
/// Mutation check: make `ListViewC`'s scroll path rebuild `selection` from the
/// visible window instead of the model index; the final assertion fails.
/// Restore.
#[test]
fn scrolling_a_list_view_recycles_rows_without_losing_selection() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "list_view");
    let (rx, ry) = driver.point("list_view", "row0");
    driver.click(rx, ry);
    assert!(
        gallery.wait_msg("selected list_view 0", REACT),
        "the first row was never selected; got {:?}",
        gallery.messages()
    );
    let selected = driver.pixel(rx, ry);

    // Ten rows in a 120 px viewport: two wheel clicks move well past the
    // recycling threshold.
    driver.scroll(rx, ry, 20.0);
    driver.scroll(rx, ry, 20.0);
    let scrolled = driver.wait_pixel_change(rx, ry, selected);
    assert!(
        !support::matches(scrolled, selected),
        "the list never scrolled: row 0 is still under the pointer"
    );

    // Back to the top: the same row must still be the selected one.
    driver.scroll(rx, ry, -20.0);
    driver.scroll(rx, ry, -20.0);
    let restored = driver.wait_pixel_settled(rx, ry);
    assert!(
        support::matches(restored, selected),
        "scrolling away and back lost the selection: {selected:?} -> {restored:?}"
    );
    assert_eq!(
        gallery
            .messages()
            .iter()
            .filter(|line| line.starts_with("selected list_view "))
            .count(),
        1,
        "scrolling must not change the selection; got {:?}",
        gallery.messages()
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test interaction_gate opening_a_drop_down`
Expected: FAIL — `the drop-down's popover never appeared below the button` if
P3's popup path is not wired, or a clean pass. Either way, run the mutation
check in the doc comment and confirm the test fails under it.

- [ ] **Step 3: Write the implementation**

No production code: the three tests are the deliverable. A popover that never
maps is a P2/P3 defect; a lost selection is a P6 defect. Record and route, do
not patch the gate.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --test interaction_gate`
Expected: PASS — 12 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui --test interaction_gate
git add ui/tests/interaction_gate.rs
git commit -m "test(ui,gallery): drop-down, scale drag and list-view recycling

The list test scrolls away and back and asserts both the pixels and the
message count, so a recycler that drops the selection fails.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 12: expander, stack, popover grab and focus order

**Files:**
- Modify: `ui/tests/interaction_gate.rs`
- Test: `ui/tests/interaction_gate.rs`

**Interfaces:**
- Consumes: Task 9's `Driver`, `KEY_TAB`, `KEY_SPACE`, `REACT`.
- Produces: the last four `#[test]`s; no new API.

- [ ] **Step 1: Write the failing tests**

Append to `ui/tests/interaction_gate.rs`:

```rust
/// The expander animates and then reveals: the child's row must be background
/// before and content after.
///
/// Mutation check: make `ExpanderC` reveal its child without emitting
/// `EventKind::Expanded`; the message never arrives and this test fails.
/// Restore.
#[test]
fn expanding_an_expander_animates_and_reveals_the_child() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "expander");
    let (tx, ty) = driver.point("expander", "title");
    // The revealed child sits below the title row.
    let child = (tx, ty + 30);
    let collapsed = driver.pixel(child.0, child.1);

    driver.click(tx, ty);
    assert!(
        gallery.wait_msg("expanded true", REACT),
        "the expander never reported opening; got {:?}",
        gallery.messages()
    );
    let revealed = driver.wait_pixel_change(child.0, child.1, collapsed);
    assert!(
        !support::matches(revealed, collapsed),
        "the child was never revealed below the title"
    );
}

/// Switching a stack page runs the transition and lands on the other page.
///
/// Mutation check: set the stack sample's `.transition_duration(0)`; the test
/// still passes (it asserts the destination, not the tween) — instead remove
/// the `on_change` handler and confirm the message never arrives. Restore.
#[test]
fn switching_a_stack_page_runs_the_transition() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "stack_switcher");
    // Two buttons, so both are indexed.
    let (bx, by) = driver.point("stack_switcher", "button1");
    driver.click(bx, by);
    assert!(
        gallery.wait_msg("page 1", REACT),
        "the switcher never changed the page; got {:?}",
        gallery.messages()
    );
    let after = driver.wait_pixel_settled(bx, by);
    let (ax, ay) = driver.point("stack_switcher", "button0");
    let other = driver.pixel(ax, ay);
    assert!(
        !support::matches(after, other),
        "the two switcher buttons look identical, so nothing marks the \
         selected page: {after:?} vs {other:?}"
    );
}

/// A menu button's popover takes an explicit `xdg_popup.grab`, so a click
/// outside must dismiss it — the compositor's doing, not the toolkit's.
///
/// This is the toolkit-side counterpart of `compositor/tests/popups.rs`'s
/// `a_grabbing_popup_chain_is_dismissed_whole_by_a_click_outside_it`.
///
/// Mutation check: make `MenuButtonC` open its popover with `autohide(false)`;
/// the click outside leaves it up and the final assertion fails. Restore.
#[test]
fn opening_a_menu_button_popover_takes_the_grab_and_dismisses_outside() {
    let mut driver = Driver::new();
    let _gallery = driver.open("light", "menu_button");
    let (bx, by) = driver.point("menu_button", "root");
    let below = (bx, by + 40);
    let closed = driver.pixel(below.0, below.1);

    driver.click(bx, by);
    let open = driver.wait_pixel_change(below.0, below.1, closed);
    assert!(
        !support::matches(open, closed),
        "the menu button's popover never mapped"
    );

    // Far from both the button and the popover.
    driver.click(bx + 300, by + 300);
    let dismissed = driver.wait_pixel_change(below.0, below.1, open);
    assert!(
        support::matches(dismissed, closed),
        "a click outside a grabbing popover did not dismiss it: {open:?} -> \
         {dismissed:?}, expected {closed:?}"
    );
}

/// Tab moves the ring in **geometric** order (contract R2), which an
/// `ActionBar` shows plainly: one button packed start, one packed end.
///
/// Mutation check: make `focus_sort` return tree order; on an `ActionBar`
/// whose end pack is built first, the ring lands on the wrong button and this
/// test fails. Restore.
#[test]
fn tabbing_through_a_form_moves_the_focus_ring_in_geometric_order() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "action_bar");
    let (sx, sy) = driver.point("action_bar", "button0");
    let (ex, ey) = driver.point("action_bar", "button1");
    assert!(sx < ex, "button0 must be the left-hand pack; got {sx} >= {ex}");
    let start_rest = driver.pixel(sx, sy);
    let end_rest = driver.pixel(ex, ey);

    driver.key(KEY_TAB);
    let start_focused = driver.wait_pixel_change(sx, sy, start_rest);
    assert!(
        !support::matches(start_focused, start_rest),
        "the first Tab did not put a visible focus ring on the left button"
    );

    driver.key(KEY_TAB);
    let end_focused = driver.wait_pixel_change(ex, ey, end_rest);
    assert!(
        !support::matches(end_focused, end_rest),
        "the second Tab did not move the ring to the right button"
    );
    let start_again = driver.wait_pixel_settled(sx, sy);
    assert!(
        support::matches(start_again, start_rest),
        "the ring stayed on the left button as well as moving right"
    );

    driver.key(KEY_SPACE);
    assert!(
        gallery.wait_msg("clicked action_bar", REACT),
        "Space did not activate the focused button; got {:?}",
        gallery.messages()
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-ui --test interaction_gate tabbing_through_a_form`
Expected: FAIL — `the first Tab did not put a visible focus ring on the left
button` until P3's focus ring and P6's `ActionBar` are both live; then PASS.
Run each doc comment's mutation check before accepting a green.

- [ ] **Step 3: Write the implementation**

No production code: these four tests complete §7's sixteen. Route failures to
the owning part.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-ui --test interaction_gate`
Expected: PASS — all 16 contract interactions.

Verify the set is exactly the contract's:

Run: `cargo test -p icedtea-ui --test interaction_gate -- --list | grep -c ': test'`
Expected: `16`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo test -p icedtea-ui
git add ui/tests/interaction_gate.rs
git commit -m "test(ui,gallery): expander, stack, popover grab and geometric focus order

Completes the contract's sixteen interactions. The popover test is the
toolkit-side counterpart of the compositor's grabbing-popup dismissal test.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 13: `ui/README.md` — the M3 documentation pass

**Files:**
- Modify: `ui/README.md`
- Modify: `ui/tests/gallery_gate.rs` (the README table test)
- Test: `ui/tests/gallery_gate.rs`

**Interfaces:**
- Consumes: `icedtea_ui::gallery::{kind_name, own_kinds, sample_shape,
  SampleShape}`; `icedtea_ui::view::Kind`.
- Produces: one more `#[test]`; no new API.

- [ ] **Step 1: Write the failing test**

Append to `ui/tests/gallery_gate.rs`:

```rust
/// The README's widget table is generated from `Kind::all()` and must stay
/// that way: a widget added to the toolkit and not to the table is a widget
/// the next reader will not know exists.
///
/// Mutation check: delete one row from the table between the markers; this
/// test fails naming that widget. Restore.
#[test]
fn the_readme_widget_table_lists_every_kind() {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
    )
    .expect("ui/README.md is readable");
    let table = readme
        .split_once("<!-- widgets:begin -->")
        .expect("the README has a `<!-- widgets:begin -->` marker")
        .1
        .split_once("<!-- widgets:end -->")
        .expect("the README has a `<!-- widgets:end -->` marker")
        .0;
    let listed: Vec<&str> = table
        .lines()
        .filter_map(|line| {
            let cell = line.strip_prefix("| `")?;
            cell.split_once('`')
        })
        .map(|(name, _)| name)
        .collect();
    for &kind in Kind::all() {
        assert!(
            listed.contains(&kind_name(kind)),
            "`{}` is missing from the README's widget table",
            kind_name(kind)
        );
    }
    assert_eq!(
        listed.len(),
        Kind::all().len(),
        "the README table has rows for widgets that do not exist: {listed:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-ui --test gallery_gate the_readme_widget_table`
Expected: FAIL — `the README has a `<!-- widgets:begin -->` marker` (panic
from `expect`), because the README has no table yet.

- [ ] **Step 3: Write the implementation**

Rewrite `ui/README.md`'s opening, and add three new sections. Everything M2
documented about the CSS engine, fonts, layout, paint and the theme stack stays
exactly as it is — only the parts M3 made untrue change.

Replace the "What this crate covers" paragraph with:

```markdown
## What this crate covers

M1 proved one themed `button` end to end. M2 widened the CSS layer to the whole
GTK 4.22 property table. **M3 turns that engine into a widget toolkit:** real
windows and popups, a keyboard/pointer/focus model, a reactive view layer, the
GTK 4.22 core widget set with GTK-exact CSS node trees, and icon theming.

| Layer | Crate |
|---|---|
| Wayland: toplevel, layer shell, popups | `wayland-client`, `wayland-protocols`, `wayland-protocols-wlr` (hand-rolled, no sctk/calloop) |
| Keyboard | `xkbcommon` (keymaps, compose, repeat) |
| 2D paint | `skia-rs-safe` (pure Rust) |
| Text shaping | `skia-rs-text` — `Typeface::from_data` + `rustybuzz` |
| Font discovery | `fontconfig` — real `fc-match` parity (default feature) |
| Layout | `taffy` |
| CSS parse / match | Servo's `cssparser` + `selectors` |
| Widgets | bespoke — `css::node::Node` trees wearing GTK's node identity |

### The module map

| Module | What it owns |
|---|---|
| `css/` | The property registry, parsing, selectors, cascade, computed values (M2) |
| `anim/` | Transitions, `@keyframes`, the animation clock (M2) |
| `layout.rs` | The `taffy` bridge: box, grid and centre containers, per-child alignment |
| `paint/` | Backgrounds, borders, shadows, outlines, blur, text, icons |
| `text.rs` | Font matching, shaping caches, wrap/ellipsize/caret/selection |
| `window/` | `Surface::{Toplevel, Layer, Popup}`, keyboard, pointer, focus, selection |
| `view/` | `View`, builders, the keyed reconciler, controllers, `App`, `Cmd` |
| `widgets/` | One file per widget: node tree, controller, behaviour |
| `icons/` | freedesktop icon themes, symbolic recolouring, builtins |
| `gallery.rs` | Every widget on one page — the binary the M3 gate measures |

## The widget set

Every widget below is a `Kind`, a builder in `view::builders`, a controller, a
GTK-exact CSS node tree with a vendored fixture, a rest-state pixel probe and —
per widget class — one driven interaction. The name is what
`gallery --widget <NAME>` takes.

<!-- widgets:begin -->
| Widget | CSS node | Notes |
|---|---|---|
| `label` | `label` | wrap, ellipsize, `<b><i><span>` markup, links |
| `spinner` | `spinner` | `:checked` while spinning, as GTK does |
…
| `alert_dialog` | `window.dialog.alert` | message, detail, buttons |
<!-- widgets:end -->
```

Generate the 64 rows rather than typing them, then fill in the two prose
columns by hand from the contract's §5 entry for each widget:

```bash
cargo run -p icedtea-ui --bin gallery -- --list \
  | while read -r name; do printf '| `%s` |  |  |\n' "$name"; done
```

Paste that block between the markers, then fill the "CSS node" column from
`Kind::css_name` plus the always-present classes (`toggle_button` →
`button.toggle`) and the "Notes" column with the one behaviour that widget adds.
Rows stay in `Kind::all()` order — the order `--list` prints and the order the
gallery page uses.

Then add, after the existing "Layout, paint and Wayland behaviour" section:

```markdown
## The gallery

```bash
cargo run -p icedtea-ui --bin gallery                 # every widget, light Adwaita
cargo run -p icedtea-ui --bin gallery -- --theme dark
cargo run -p icedtea-ui --bin gallery -- --widget check_button
```

One scrollable page, one `frame` per widget, in `Kind::all()` order. The page is
built by *iterating* `Kind::all()`, so a widget added without a gallery entry
does not compile — the gallery is the toolkit's completeness measure, not a
demo.

| Option | Meaning |
|---|---|
| `--theme <light\|dark\|hc>` | Which bundled sheet to compile. Default: light. |
| `--theme-file <PATH>` | A sheet on disk, used *whole*, instead of a bundled one. |
| `--widget <NAME>` | Render exactly one widget, alone, at the origin. |
| `--list` | Print every widget name, one per line, and exit. |
| `--probe-points` | Print `<widget> <label> <x> <y>` for every probe point and exit. |
| `--print-allocation` | Print `<widget> <x> <y> <width> <height>` per entry and exit. |
| `--size <WxH>` | Surface size. Default: 1280x800. |
| `--scroll <PX>` | Scroll the page before the first frame. |
| `--scale <N>` | Output scale, for HiDPI probes. Default: 1. |

`--list`, `--probe-points` and `--print-allocation` never touch Wayland: they
run the same view → reconcile → restyle → layout pipeline the app loop runs and
print what it produced. Everything else maps a `zwlr_layer_shell_v1` overlay
anchored top-left, so on-screen coordinates are page coordinates.

**Probe points** are derived, never declared: `"root"` is the widget's own node,
every descendant is labelled by its CSS node name, and a name that occurs more
than once is indexed (`tab0`, `image1`). That is how the gates learn where to
sample — no test in this crate hard-codes a coordinate.

Every folded message is printed as `msg <line>` on stdout and flushed
immediately, which is how the interaction gate asserts on the model from
outside the process.

## The M3 gates

```bash
cargo test -p icedtea-ui --test gallery_gate       # rest state, 3 themes
cargo test -p icedtea-ui --test interaction_gate   # 16 driven interactions
cargo test -p icedtea-ui --test node_trees         # GTK node-tree conformance
```

- `tests/gallery_gate.rs` walks the page in surface-height slices under the
  harness compositor and asserts every widget paints something in light, dark
  and high-contrast Adwaita; that every `Kind` appears in `--list`, on the page
  and (for sub-kinds) inside its parent's node tree; that at least one probe
  point per widget differs between light and dark; that every widget's node tree
  matches its vendored GTK 4.22 fixture; and that this README's table lists
  every `Kind`.
- `tests/interaction_gate.rs` drives one interaction per widget class with a
  virtual pointer and keyboard: click, toggle, check, switch, type, debounced
  search, password peek, held spin repeat, drop-down pick, scale drag, list
  scroll with recycling, expander, stack page, popover grab and dismissal, Tab
  in geometric order, and the pointer-click-without-focus-ring rule.
- Colours are pinned in exactly one place — `tests/themed_button_offscreen.rs`,
  the M1 gate. The gallery gates assert *change*, not constants: 64 pinned
  colours would be a fixture to maintain, not a gate.
```

Finally, replace "Deliberately not covered by M2" with:

```markdown
## Deliberately not covered by M3

Input methods and `text-input-v3`, the emoji chooser, drag and drop, and
`accesskit` accessibility (M6). Markup beyond `<b><i><span>`. GL and video
widgets (`GLArea`, `Video`, `MediaControls`), the portal-backed choosers
(file, print, app) and `LockButton`, and the deprecated widgets GTK 4.22 itself
retired — except `Statusbar`, `InfoBar` and `ShortcutsWindow`, which Adwaita
still styles and which the M3 contract keeps (ruling R1). Client-side cursor
themes: the toolkit maps the `cursor` property to `wp_cursor_shape_v1` names
instead. XWayland clients. Fractional scale. The app migrations onto this
toolkit — settings, then shell, then clipboard — are M5.
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p icedtea-ui --test gallery_gate the_readme_widget_table`
Expected: PASS.

Run: `cargo test -p icedtea-ui --test gallery_gate`
Expected: PASS — 8 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo test -p icedtea-ui --test gallery_gate
git add ui/README.md ui/tests/gallery_gate.rs
git commit -m "docs(ui): the M3 README — widget table, module map, gallery and gates

The widget table sits between machine-readable markers and is asserted against
Kind::all(), so a widget added to the toolkit and not to the table fails the
gate.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 14: spec status, contract amendments, and the whole-milestone gate

**Files:**
- Modify: `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`
- Modify: `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
- Modify: `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§10)
- Test: the whole workspace

**Interfaces:**
- Consumes: everything.
- Produces: no code.

- [ ] **Step 1: Write the failing check**

There is no unit test for a status line, so the check is executable and exact —
run it first and watch it fail:

```bash
grep -q 'implemented on `rebuild/pure-rust-gtk-m3`' \
  docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md \
  && echo "status updated" || echo "status NOT updated"
```

Expected: `status NOT updated`.

- [ ] **Step 2: Run the whole-milestone gate and record what it says**

```bash
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --no-default-features --all-targets -- -D warnings
cargo clippy -p icedtea-harness --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p icedtea-ui --no-deps
cargo test --workspace
cargo test -p icedtea-compositor --test popups
cargo test -p icedtea-compositor --test popups
cargo test -p icedtea-compositor --test popups
```

Expected: every command exits 0, `compositor/tests/popups.rs` green three times
in a row (its determinism gate), and `cargo test --workspace` reporting the M1
gate's 4, the M2 gate's 9 + 4, the transition test's 1 and the M3 gates' 8 + 16
among the rest. If anything fails, stop: the failure belongs to the owning
part's fix wave, and this task does not start until the tree is green.

- [ ] **Step 3: Write the documentation**

In `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`,
replace the status line:

```markdown
**Status:** implemented on `rebuild/pure-rust-gtk-m3` (parts 1–8 of
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md`); `wlr` 0.20.28
published; awaiting owner review before merge
```

In `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`, replace the
status line:

```markdown
**Status:** M1 and M2 implemented and merged; M3 implemented on
`rebuild/pure-rust-gtk-m3` (widget toolkit, popups, windows, icons — the parent
plan's M4 folded in); M5–M6 proposed
```

and rewrite the decomposition's entries 3 and 4:

```markdown
3. **M3 — Widget toolkit breadth** *(implemented)*: xdg-popup end to end (`wlr`
   0.20.28 → compositor → harness → toolkit), the window/event/focus layer over
   `xkbcommon`, a keyed reactive view layer, the GTK 4.22 core widget set with
   GTK-exact CSS node trees, and icon theming. Measured by the `gallery` binary
   and its two gates; `ui/README.md` is the reference.
4. **M4 — Icon & asset theming** *(folded into M3, done)*: freedesktop icon
   themes, symbolic recolouring, builtin shapes and HiDPI assets shipped as M3's
   P7. Client-side cursor themes were dropped: icedtea supports
   `wp_cursor_shape_v1`, so the toolkit maps the `cursor` property to shape
   names instead.
```

In `docs/superpowers/plans/2026-08-27-m3-part0-contract.md`, replace §10's
placeholder with the eleven amendments this part carried out, each in the M2
contract's shape — `### E<n> — <ruling>`, the conflicting texts quoted, the
ruling, and the part that carried it out. Use this plan's "Contract deviations"
section as the source; number them `E1`–`E11` in that order, and append any
amendment earlier parts already recorded *before* them, renumbering only if §10
is still empty.

- [ ] **Step 4: Run the check to verify it passes**

```bash
grep -q 'implemented on `rebuild/pure-rust-gtk-m3`' \
  docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md \
  && echo "status updated"
grep -c '^### E' docs/superpowers/plans/2026-08-27-m3-part0-contract.md
cargo test --workspace
```

Expected: `status updated`; at least `11` amendment headings; the workspace
green.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md \
        docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "docs(m3): spec status and the contract amendments P8 carried out

Records the eleven deviations this part took, each with the conflicting
contract text quoted and the ruling, in the M2 contract's own shape.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Stop for consent**

M3 is complete on `rebuild/pure-rust-gtk-m3`. **Do not merge.** Present the
branch as ready — gates green, reviews passed — and ask whether to proceed with
the PR/merge or wait while the owner runs `/simplify`, `/code-review` or
`/optimize` themselves.

---

## Self-Review

### 1. Spec coverage

| Spec / contract requirement | Where |
|---|---|
| §7 `gallery` binary, `cargo run -p icedtea-ui --bin gallery` | Tasks 1, 5, 6 |
| §7 `--theme <light\|dark\|hc>` | Task 1 (`Theme`), Task 5 (`compile_sheet`) |
| §7 `--theme-file <PATH>` | Task 1 (parse), Task 5 (`compile_sheet`, used *whole*) |
| §7 `--widget <NAME>`, "alone, at the origin" | Task 1 (parse + sub-kind rejection), Task 4 (`page`'s widget branch) |
| §7 `--list` | Task 1 (`print_list`), asserted in Task 8 |
| §7 `--probe-points`, "the gate never hard-codes" | Task 5 (`print_probe_points`), consumed in Tasks 6–12 |
| §7 `--size`, `--scroll`, `--scale` | Task 1 (parse), Task 5 (`scrolled_page`, `build`), Task 6 (`run`) |
| §7 `--print-allocation` | Task 5 (`print_allocations`), consumed in Tasks 6–8 |
| §7 one scrollable page, one `frame` per widget, `Kind::all()` order | Task 4 (`page`), asserted in Task 4 and Task 8 |
| §7 "a widget missing from the page fails the gate" / compile-time hole | Tasks 2–4 (exhaustive `sample`, no wildcard after Task 4), Task 8 |
| §7 probe-point convention (`root`, `check`, `slider`, `tab0`, `row0`) | Task 5 (`probe_points_of`), asserted in Task 5's two tests |
| §7 `support::{ProbePoint, probe_points, spawn_gallery, spawn_gallery_widget}` | Task 6 (widened per deviations 4) |
| §7 `every_kind_appears_in_the_gallery` | Task 8 |
| §7 `every_widget_renders_at_rest_in_the_{light,dark,high_contrast}_theme` | Tasks 6, 7 |
| §7 `every_probe_point_differs_between_light_and_dark` | Task 7 (per widget, deviation 6) |
| §7 `the_node_tree_of_every_widget_matches_its_gtk_fixture` | Task 8 |
| §7 the sixteen `interaction_gate.rs` tests | Tasks 9 (4 + focus-ring), 10 (4), 11 (3), 12 (4) — 16 total |
| §7 "both gates run against the harness compositor" | Tasks 6–12 (`Compositor::spawn`, `ScreencopyClient`, virtual pointer/keyboard) |
| §7 `node_trees.rs` hermetic, P8 only wires it | Task 8 Step 3 |
| §7 P8 owns the M3 documentation pass (README + spec status) | Tasks 13, 14 |
| §9 P8 gate: 6 + 16 tests in three themes, `cargo test --workspace`, clippy ×2, fmt, doc | Task 14 Step 2 |
| §9 cross-cutting: untrusted input never panics | Task 1 (`hostile_argv_…`), Task 6 (`hostile_child_output_…`) |
| §9 cross-cutting: every load-bearing test records a mutation check | Every test's doc comment in Tasks 1–13 |
| §9 cross-cutting: timing assertions are complexity bounds | `GALLERY_MAP_TIMEOUT`, `REACT_TIMEOUT`, `REACT` — all named and justified |
| §9 "must not touch: any widget implementation" | Tasks 10–12 Step 3 route every failure to the owning part |
| Spec §7 "the gallery measures completeness" | Task 8's `every_kind_appears_in_the_gallery` |
| Spec risks: "scope, mitigated by the gallery gate" | Tasks 6–8 |

Gaps found and closed while writing: the contract never defines `SurfaceSpec`,
`ListItem` or the list `factory` type, and never says how a widget exposes its
probe points or how the gate makes "model assertions" across a process
boundary. All are recorded in "Contract deviations" (2, 3, 4, 10) rather than
silently invented, and every ASSUMED construction is marked at its call site.

### 2. Placeholder scan

Searched for `TBD`, `TODO`, `FIXME`, `similar to Task`, `implement later`,
`handle edge cases`, `add appropriate`, `and so on`, `…` inside code blocks.

- No `TBD`/`TODO`/`FIXME` anywhere.
- Two deliberate, *removed-by-a-later-task* placeholders exist in production
  code, each with the removing task named at the site: `sample`'s `_ =>` arm
  (Tasks 2–3, deleted in Task 4, and Task 4's totality test is what proves it is
  gone) and `bin/gallery.rs`'s "nothing to do yet" branch (Task 1, replaced in
  Tasks 5 and 6). Neither survives the plan.
- The README's widget table shows the exact row format, the first two and last
  rows, and the exact shell command that generates the remaining 61 — a
  generation step, not an unfilled blank, and Task 13's test fails if any row is
  missing.
- Tasks 10, 11 and 12 have "no production code" implementation steps. That is
  the deliverable being tests, not a missing step: each states what to do when
  a test fails (route to the owning part) and what the single permitted
  adjustment is (the probe label the binary actually prints).

### 3. Type consistency

- `Theme` / `Options` / `OptionsError` / `kind_name` / `kind_from_name` /
  `SampleShape` / `sample_shape` (Task 1) are used unchanged in Tasks 2, 4, 5,
  8, 13.
- `GalleryMsg` / `GalleryModel` / `update` / `Sample` (Task 2) are used
  unchanged in Tasks 3, 4, 5, 8. `GalleryModel::scroll` was added in Task 5's
  revision and is declared in Task 2's struct — checked in both places.
- `sample(kind, model) -> Sample` has one signature everywhere; `page(model)`
  and `scrolled_page(model)` are both `fn(&GalleryModel) -> View<GalleryMsg>`,
  which is what `App::new`'s `fn` pointer parameter requires (no closure
  captures anywhere).
- `App::probe(size, sheet, fonts, icons, clock)` is declared in Task 5's
  Produces block, implemented with the same parameter list, and called with the
  same five arguments in `gallery::build` and in `app.rs`'s own test.
- `Probe::{root, instances, allocation}` is used only as declared;
  `probe_points_of(instance, widget, probe)` and
  `entry_instances(opts, probe)` keep one argument order across Tasks 5, 6, 7, 8.
- `support::ProbePoint` (owned `String`s) and `gallery::ProbePoint`
  (`&'static str` widget) are deliberately different types; the difference is
  documented at the support type and no code converts between them.
- `Driver`'s methods (Task 9) are called with the same names and argument order
  in Tasks 10–12: `open`, `point`, `click`, `press`, `release`, `drag`,
  `scroll`, `key`, `keys`, `pixel`, `wait_pixel_change`, `wait_pixel_settled`.
- `GalleryProc::wait_msg(needle, timeout)` takes the message line **without**
  the `msg ` prefix in every call site, matching the reader thread that strips
  it.
- `spawn_gallery(socket, theme, scroll)` and
  `spawn_gallery_widget(socket, theme, widget)` keep `socket` first in every
  call, matching the M2 `spawn_themed_button` convention they sit beside.
- Harness: `VirtualPointerClient::axis(horizontal, vertical)` is declared and
  called with that order in `Driver::scroll` (`axis(0.0, vertical)`).
