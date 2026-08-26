# Pure-Rust GTK-themed UI — M1 Proving Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the whole pure-Rust UI pipeline through one narrow path — a single GTK-themed `button`, rendered offscreen with pixel assertions and on-screen as a `zwlr_layer_shell_v1` overlay — so every seam (Wayland → CSS parse → selector match → cascade → computed style → taffy layout → skia paint → text) is exercised before M2 scales breadth.

**Architecture:** A new workspace crate `icedtea-ui` at `ui/`. CSS comes in as text (a vendored copy of GTK4's Adwaita `Default-light.css`, or the user's own `gtk.css` at runtime), is tokenized by `cssparser` 0.37 into `(selector-text, declarations)` plus an `@define-color` table, and each rule's prelude is compiled to a `selectors` 0.40 `SelectorList<GtkSelectorImpl>`. A `CssNode` — element name, style classes, pseudo-class state, parent chain — implements `selectors::Element`, so Servo's matcher drives selection unmodified; the cascade orders matched declarations by `(important, specificity, source order)` and produces a `ComputedStyle`. `taffy` 0.14 turns padding/border/min-size plus the label's intrinsic size into an allocation rect, and `skia-rs-safe` 0.4 (the repo owner's **pure-Rust** Skia reimplementation — no C++ build) paints a rounded-rect background + border + glyph runs into a raster `Surface`. **Text stack decision: `skia-rs-text`, not `cosmic-text`.** `skia_rs_text::Typeface::from_data(Vec<u8>)` loads a TTF/OTF straight from disk bytes (parsed by `ttf_parser`), `Shaper::shape_auto(text, &Font)` shapes via `rustybuzz` and returns per-glyph advances/offsets, `TextBlobBuilder::add_positioned_run` assembles a blob, and `Canvas::draw_text_blob` rasterizes it through `Font::glyph_path`. That covers load-from-disk + shape + measure + draw with zero extra crates, so `cosmic-text` is dropped from M1 (the spec's "this may collapse (resolved in M1)" — it collapses). Fonts are found fontconfig-free by probing a fixed list of well-known paths. Skia's raster `Surface` cannot wrap external pixel memory (`Surface::new_raster` always allocates its own `PixelBuffer`, always physically RGBA-premultiplied), so the Wayland seam is an explicit RGBA→BGRA swizzling copy into an `Argb8888` `wl_shm` buffer.

**Tech Stack:** `skia-rs-safe 0.4.0` (features `std`, `text`), `cssparser 0.37.0`, `selectors 0.40.0`, `precomputed-hash 0.1`, `taffy 0.14.0`, `wayland-client 0.31`, `wayland-protocols 0.32`, `wayland-protocols-wlr 0.3`, `rustix 1` (fs), `tracing 0.1`; dev: `icedtea-harness` (path).

**Spec:** docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md (§Milestone 1)

## Global Constraints

- New crate name is `icedtea-ui`, directory `ui/`, added to the root `Cargo.toml` `members` list.
- Crate versions are pinned exactly: `skia-rs-safe = { version = "0.4.0", default-features = false, features = ["std", "text"] }`, `cssparser = "0.37"`, `selectors = "0.40"`, `precomputed-hash = "0.1"`, `taffy = "0.14"`, `wayland-client = "0.31"`, `wayland-protocols = { version = "0.32", features = ["client"] }`, `wayland-protocols-wlr = { version = "0.3", features = ["client"] }`, `rustix = { version = "1", features = ["fs"] }`.
- No Smithay: no `smithay`, no `smithay-client-toolkit`, no `calloop` may appear in `ui/Cargo.toml`.
- No GNOME toolkit: no `gtk4`, `gio`, `glib`, `gobject`, `pango`, `cairo`, `gdk`, `gtk4-layer-shell` may appear in `ui/Cargo.toml`.
- No `cosmic-text`: the text stack is `skia-rs-text` (re-exported as `skia_rs_safe::text`).
- `edition = "2024"`, `rust-version = "1.94"` — both inherited via `edition.workspace = true` / `rust-version.workspace = true`.
- Logging is `tracing` (`tracing.workspace = true`); tests are plain `#[test]`, in-crate unit tests plus `ui/tests/*.rs` integration tests.
- `wl_shm` format is `wl_shm::Format::Argb8888`: 4 bytes/pixel, byte order in memory `B, G, R, A` (little-endian `0xAARRGGBB`), **alpha premultiplied**. Stride is `width * 4`. Skia's `PixelBuffer` is physically `R, G, B, A` premultiplied with the same stride, so the copy is a per-pixel R/B swap.
- Every commit message ends with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Gates, all of which must pass before a task's commit:
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo fmt --all --check`
- The **load-bearing test** is `ui/tests/themed_button_offscreen.rs::adwaita_button_computed_style_and_pixels_match_the_theme`. It is the M1 gate; the layer-shell binary and `ui/tests/layer_shell_screencopy.rs::themed_button_paints_accent_blue_on_a_layer_surface` prove the Wayland seam.

## File Structure

| File | Single responsibility |
|---|---|
| `Cargo.toml` (root, modified) | Add `"ui"` to `members`. |
| `ui/Cargo.toml` | Crate manifest: the pinned dependency set above; `[[bin]] themed-button`. |
| `ui/themes/adwaita-light.css` | Verbatim vendored copy of GTK 4's `/org/gtk/libgtk/theme/Default/Default-light.css`, so tests are hermetic. |
| `ui/themes/README.md` | Provenance + LGPL-2.1-or-later license note for the vendored CSS. |
| `ui/src/lib.rs` | Crate root: module declarations and the few re-exports the binary and tests use. |
| `ui/src/css/mod.rs` | `css` module root; re-exports `parse`, `colors`, `select`, `cascade`, `computed`. |
| `ui/src/css/parse.rs` | `cssparser` glue: text → `Stylesheet { rules: Vec<StyleRule>, colors: ColorTable }`. Nothing about matching or values. |
| `ui/src/css/colors.rs` | `ColorTable` and `parse_color_value` — `@name` lookup layered over `skia_rs_core::Color::from_css`. |
| `ui/src/css/select.rs` | `GtkSelectorImpl`, `CssString`, `GtkPseudoClass`, `GtkPseudoElement`, `GtkSelectorParser`, `CssNode`/`NodeData` + its `selectors::Element` impl. |
| `ui/src/css/cascade.rs` | `CompiledSheet`, `CompiledRule`, `cascade()` — matched declarations ordered by `(important, specificity, source order)`. |
| `ui/src/css/computed.rs` | `Background`, `ComputedStyle`, and the declaration→property resolution that fills it. |
| `ui/src/text.rs` | `FontStack`: fontconfig-free system-typeface probe, shaping, measurement, blob building. |
| `ui/src/layout.rs` | `layout_button`: `ComputedStyle` + label size → `taffy` tree → `Allocation` rect. |
| `ui/src/paint.rs` | `paint_button`: draw background (solid or vertical gradient), border, and label into a `Surface`. |
| `ui/src/widget/mod.rs` | `widget` module root. |
| `ui/src/widget/button.rs` | `Button`: node identity + `PseudoStates` + `restyle()`/`measure()`/`render()` — the retained widget. |
| `ui/src/shm.rs` | `ShmBuffer`: memfd + `wl_shm_pool` + `wl_buffer` (`Argb8888`), and the RGBA→BGRA copy from a Skia `Surface`. |
| `ui/src/wayland.rs` | `LayerWindow`: registry binding, `zwlr_layer_shell_v1` overlay surface, pointer events, dispatch loop. |
| `ui/src/app.rs` | `run_themed_button`: wires theme loading + widget + `LayerWindow` into one runnable app. |
| `ui/src/bin/themed-button.rs` | The runnable binary: env handling (`ICEDTEA_UI_THEME`), `tracing` init, calls `run_themed_button`. |
| `ui/tests/themed_button_offscreen.rs` | **The load-bearing test**: computed values + pixel assertions against the vendored Adwaita CSS. |
| `ui/tests/layer_shell_screencopy.rs` | Harness round-trip: boot the compositor, run the binary, screencopy, assert accent blue is on screen. |
| `ui/README.md` | What the crate is, how to run the binary, what M1 does and does not cover. |
| `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md` (modified) | Status line flipped to record M1 as implemented. |

## Exact Adwaita values this plan asserts

Read from the vendored `ui/themes/adwaita-light.css` (line numbers are in the 1,941-line extracted file):

| Selector | Line | Declarations the slice consumes |
|---|---|---|
| `notebook > header > tabs > arrow, button` | 215 | `min-height: 24px; min-width: 16px; padding: 4px 9px; border: 1px solid; border-radius: 5px; color: #2e3436; border-color: #cdc7c2; background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa);` |
| `… , button:hover` | 221 | `color: #2e3436; border-color: #cdc7c2; background-image: linear-gradient(to top, #d6d1cd, #e8e6e3 1px);` |
| `… , button.keyboard-activating, button:active, button:checked` | 223 | `color: #2e3436; border-color: #cdc7c2; background-image: image(#dad6d2);` |
| `button.suggested-action` | 297 | `color: white; border-color: #15539e; background-image: linear-gradient(to top, #2c7fe3 2px, #3584e4);` |
| `button.suggested-action:active, …:checked` | 307 | `color: white; border-color: #185fb4; background-image: image(#1961b9);` |
| `window` | 1738 | `border-width: 0px;` |
| `@define-color theme_fg_color` | 1864 | `#2e3436` |
| `@define-color borders` | 1912 | `#cdc7c2` |
| `@define-color accent_color` | 1921 | `#3584E4` |
| `@define-color wm_highlight` | 1927 | `rgba(255, 255, 255, 0.8)` |

**The surprise that shapes this plan:** Adwaita's `button` rules carry **no `background-color`** at all — every button background is a `background-image`, either the GTK `image(<color>)` function (a flat fill) or a two-stop `linear-gradient(to top, …)`. So `ComputedStyle::background` is an enum, not a `Color`, and the pixel assertions are chosen against states whose center pixel is an exact, independently-derivable color:

- `:active` → `image(#dad6d2)` → flat; center pixel is exactly `0xFFDAD6D2`.
- `:hover` → `linear-gradient(to top, #d6d1cd, #e8e6e3 1px)`; `to top` puts the gradient line's origin at the **bottom**, so the second stop is reached 1 px up and everything above it is flat `#e8e6e3`; the center pixel is exactly `0xFFE8E6E3`.
- normal → `linear-gradient(to top, #f6f5f4 2px, #fbfafa)`; below 2 px from the bottom the fill is flat `#f6f5f4` (the pre-first-stop region), so the pixel one row above the inner bottom edge is exactly `0xFFF6F5F4`, and the center pixel differs from both `#dad6d2` and `#e8e6e3`.
- `.suggested-action` → gradient `#2c7fe3 → #3584e4`, both within 9/5/1 per-channel of `accent_color` `#3584E4` — that is the blue the screencopy test looks for, and it is far from the compositor's wallpaper `#1e1e2e` and default palette foreground `#cdd6f4`.

Adwaita's button rules use only literal `#rrggbb` / `rgba()` / the keyword `white`, never `@name`, `alpha()`, `shade()` or `mix()`. **Ruling:** M1 implements the `@define-color` table plus `@name` value references (both required by the spec) and delegates literal color syntax to `skia_rs_core::Color::from_css`; `alpha()`/`shade()`/`mix()` are M2, because no button rule needs them. `@define-color` entries whose value uses CSS relative-color syntax (`hsl(from …)`, `rgb(from black … / calc(…))` — 8 of Adwaita's 37) are recorded as unresolved and skipped, not fabricated.

---

## Task 1 — Crate scaffold, workspace member, vendored theme

**Files**
- Create: `ui/Cargo.toml`, `ui/src/lib.rs`, `ui/themes/adwaita-light.css`, `ui/themes/README.md`
- Modify: `Cargo.toml` (root)
- Test: `ui/src/lib.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: nothing.
- Produces: `pub const BUNDLED_ADWAITA_LIGHT: &str` in `ui/src/lib.rs`.

### Steps

- [ ] **Vendor the theme.** GTK ships Adwaita as a gresource inside the shared object; extract it verbatim:
  ```bash
  mkdir -p ui/themes
  gresource extract /usr/lib/libgtk-4.so.1 /org/gtk/libgtk/theme/Default/Default-light.css > ui/themes/adwaita-light.css
  wc -l ui/themes/adwaita-light.css   # expect 1941
  grep -c '@define-color' ui/themes/adwaita-light.css   # expect 37
  ```
  If the count is not 1941/37, stop: the installed GTK differs from the one this plan's expected values were read from, and every asserted color below must be re-derived from the actual file before continuing.

- [ ] **Write the license note** `ui/themes/README.md`:
  ```markdown
  # Vendored theme files

  ## `adwaita-light.css`

  Verbatim extract of GTK 4's default light theme:

      gresource extract /usr/lib/libgtk-4.so.1 \
          /org/gtk/libgtk/theme/Default/Default-light.css

  Extracted 2026-08-25 from GTK 4 as packaged on Arch Linux (1,941 lines,
  37 `@define-color` declarations).

  **License:** GTK is licensed under the GNU Lesser General Public License,
  version 2.1 or later. This file is part of GTK and is redistributed here
  under the LGPL-2.1-or-later, unmodified. See
  <https://gitlab.gnome.org/GNOME/gtk/-/blob/main/COPYING>.

  It is vendored so `icedtea-ui`'s tests are hermetic: they must assert
  against a fixed, known theme rather than whatever GTK happens to be
  installed. At runtime `icedtea-ui` prefers the user's own theme
  (`$XDG_CONFIG_HOME/gtk-4.0/gtk.css`, then
  `/usr/share/themes/<Name>/gtk-4.0/gtk.css`) and only falls back to this
  copy.
  ```

- [ ] **Write the failing test** in `ui/src/lib.rs`:
  ```rust
  //! `icedtea-ui` — a pure-Rust, GTK4-theme-compatible widget layer.
  //!
  //! M1 proving slice: one themed `button`, from a real GTK4 `gtk.css`
  //! through selector matching, cascade, `taffy` layout and `skia-rs` paint,
  //! onto a `zwlr_layer_shell_v1` surface. See
  //! `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.

  /// GTK 4's default light theme, vendored so tests are hermetic.
  ///
  /// See `ui/themes/README.md` for provenance and the LGPL-2.1-or-later
  /// note that covers it.
  pub const BUNDLED_ADWAITA_LIGHT: &str = include_str!("../themes/adwaita-light.css");

  #[cfg(test)]
  mod tests {
      use super::BUNDLED_ADWAITA_LIGHT;

      #[test]
      fn bundled_adwaita_is_the_extracted_gtk4_theme() {
          assert_eq!(
              BUNDLED_ADWAITA_LIGHT.lines().count(),
              1941,
              "vendored Adwaita is not the 1,941-line GTK4 Default-light.css this crate's \
               expected values were derived from"
          );
          assert_eq!(
              BUNDLED_ADWAITA_LIGHT.matches("@define-color").count(),
              37,
              "vendored Adwaita does not carry GTK4's 37 @define-color declarations"
          );
          assert!(
              BUNDLED_ADWAITA_LIGHT
                  .contains("background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa)"),
              "vendored Adwaita is missing the base `button` background this crate asserts on"
          );
      }
  }
  ```

- [ ] **Run it — expect a compile failure**, because the crate is not in the workspace yet:
  ```bash
  cargo test -p icedtea-ui
  ```
  Expected: `error: package ID specification 'icedtea-ui' did not match any packages`.

- [ ] **Write `ui/Cargo.toml`:**
  ```toml
  [package]
  name = "icedtea-ui"
  version.workspace = true
  edition.workspace = true
  rust-version.workspace = true

  # A pure-Rust, GTK4-theme-compatible UI layer: no gtk4/gio/glib/pango/cairo/gdk,
  # and no Smithay (these are Wayland *clients*; wlroots is the server side).

  [dependencies]
  skia-rs-safe = { version = "0.4.0", default-features = false, features = ["std", "text"] }
  cssparser = "0.37"
  selectors = "0.40"
  precomputed-hash = "0.1"
  taffy = "0.14"
  wayland-client = "0.31"
  wayland-protocols = { version = "0.32", features = ["client"] }
  wayland-protocols-wlr = { version = "0.3", features = ["client"] }
  rustix = { version = "1", features = ["fs"] }
  tracing.workspace = true
  tracing-subscriber.workspace = true

  [dev-dependencies]
  icedtea-harness = { path = "../harness" }

  [[bin]]
  name = "themed-button"
  path = "src/bin/themed-button.rs"
  ```

- [ ] **Add the workspace member** in the root `Cargo.toml`:
  ```toml
  members = ["contract", "config", "compositor", "clipboard", "harness", "shell", "settings", "notifications", "ui"]
  ```

- [ ] **Create the binary stub** `ui/src/bin/themed-button.rs` so `[[bin]]` resolves; it is filled in by Task 11:
  ```rust
  //! `themed-button` — the M1 proving-slice binary. Filled in by Task 11.

  fn main() {
      eprintln!("themed-button: not yet implemented (M1 Task 11)");
      std::process::exit(1);
  }
  ```

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
  Expected: `test tests::bundled_adwaita_is_the_extracted_gtk4_theme ... ok`, `1 passed`.

- [ ] **Commit:**
  ```bash
  git add Cargo.toml Cargo.lock ui/Cargo.toml ui/src/lib.rs ui/src/bin/themed-button.rs ui/themes/adwaita-light.css ui/themes/README.md
  git commit -m "$(cat <<'EOF'
  feat(ui): scaffold icedtea-ui with a vendored Adwaita theme

  New workspace crate for the pure-Rust, GTK4-theme-compatible UI layer
  (spec §Milestone 1). Vendors GTK 4's Default-light.css verbatim
  (LGPL-2.1-or-later, provenance in ui/themes/README.md) so the CSS tests
  are hermetic rather than hostage to the installed GTK.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2 — Parse a stylesheet into rules and an `@define-color` table

**Files**
- Create: `ui/src/css/mod.rs`, `ui/src/css/parse.rs`
- Modify: `ui/src/lib.rs` (add `pub mod css;`)
- Test: `ui/src/css/parse.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: `BUNDLED_ADWAITA_LIGHT`.
- Produces:
  ```rust
  pub struct Declaration { pub name: String, pub value: String, pub important: bool }
  pub struct StyleRule { pub selector_text: String, pub declarations: Vec<Declaration>, pub source_order: usize }
  pub struct Stylesheet { pub rules: Vec<StyleRule>, pub color_definitions: Vec<(String, String)> }
  pub fn parse_stylesheet(css: &str) -> Stylesheet;
  ```
  `color_definitions` is the ordered list of raw `@define-color <name> <value>` pairs; Task 3 resolves it into a `ColorTable` (kept separate so parsing has no opinion about color syntax).

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/css/parse.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::{parse_stylesheet, Declaration};

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
          assert_eq!(sheet.rules.len(), 1, "@define-color must not become a style rule");
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
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui css::parse
  ```
  Expected: `error[E0583]: file not found for module 'css'`.

- [ ] **Write `ui/src/css/mod.rs`:**
  ```rust
  //! The GTK4-CSS engine: parse, color resolution, selector matching,
  //! cascade, and computed values.

  pub mod cascade;
  pub mod colors;
  pub mod computed;
  pub mod parse;
  pub mod select;
  ```
  (`cascade`, `colors`, `computed` and `select` land in Tasks 3–6; create each file empty-but-documented as its task begins. To keep this task compiling on its own, add only `pub mod parse;` now and extend the list in each later task.)

- [ ] **Add to `ui/src/lib.rs`,** directly under the crate doc comment:
  ```rust
  pub mod css;
  ```

- [ ] **Write `ui/src/css/parse.rs`.** Two parser structs are needed because `cssparser` 0.37 requires a `RuleBodyItemParser` to unify its `Declaration`/`QualifiedRule`/`AtRule` associated types, which cannot also equal the stylesheet level's rule type:
  ```rust
  //! `cssparser` glue: CSS text in, `(selector text, declarations)` rules and
  //! raw `@define-color` pairs out.
  //!
  //! Deliberately value-agnostic: declaration values are kept as their
  //! verbatim source text and interpreted later (`super::colors`,
  //! `super::computed`). That keeps every GTK-specific value form
  //! (`image()`, `-gtk-*`, relative color syntax) parseable-as-text even
  //! when this milestone cannot yet interpret it.

  use cssparser::{
      AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
      QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser,
  };

  /// One `name: value` pair from a declaration block.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Declaration {
      /// Property name, ASCII-lowercased.
      pub name: String,
      /// Verbatim value text, `!important` stripped and trimmed.
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
      /// 0-based index of this rule in the sheet; the cascade's final tiebreak.
      pub source_order: usize,
  }

  /// A parsed stylesheet.
  #[derive(Debug, Clone, Default)]
  pub struct Stylesheet {
      /// Qualified rules, in source order.
      pub rules: Vec<StyleRule>,
      /// `(name, raw value)` for every `@define-color`, in source order.
      pub color_definitions: Vec<(String, String)>,
  }

  /// Split a raw declaration value into `(value, important)`.
  ///
  /// Tolerates whitespace between `!` and `important` (`cssparser` tokenizes
  /// them as separate tokens, so the source text can carry either spelling).
  fn split_important(raw: &str) -> (String, bool) {
      let trimmed = raw.trim();
      if let Some(head) = trimmed.strip_suffix("important") {
          let head = head.trim_end();
          if let Some(head) = head.strip_suffix('!') {
              return (head.trim_end().to_string(), true);
          }
      }
      (trimmed.to_string(), false)
  }

  /// Consume every remaining token in `input` and return the source text it spanned.
  fn remaining_text<'i>(input: &mut Parser<'i, '_>) -> &'i str {
      let start = input.position();
      while input.next().is_ok() {}
      input.slice_from(start)
  }

  /// Parses a single declaration block. `RuleBodyItemParser` demands one type
  /// for declarations, nested qualified rules and nested at-rules alike; nested
  /// rules are refused (`parse_qualified` is `false`), so `Declaration` serves
  /// as all three.
  struct DeclarationBlockParser;

  impl<'i> DeclarationParser<'i> for DeclarationBlockParser {
      type Declaration = Declaration;
      type Error = ();

      fn parse_value<'t>(
          &mut self,
          name: CowRcStr<'i>,
          input: &mut Parser<'i, 't>,
          _start: &ParserState,
      ) -> Result<Declaration, ParseError<'i, ()>> {
          let (value, important) = split_important(remaining_text(input));
          Ok(Declaration {
              name: name.as_ref().to_ascii_lowercase(),
              value,
              important,
          })
      }
  }

  impl<'i> AtRuleParser<'i> for DeclarationBlockParser {
      type Prelude = ();
      type AtRule = Declaration;
      type Error = ();
  }

  impl<'i> QualifiedRuleParser<'i> for DeclarationBlockParser {
      type Prelude = ();
      type QualifiedRule = Declaration;
      type Error = ();
  }

  impl<'i> RuleBodyItemParser<'i, Declaration, ()> for DeclarationBlockParser {
      fn parse_declarations(&self) -> bool {
          true
      }

      fn parse_qualified(&self) -> bool {
          false
      }
  }

  /// Top-level rule-list parser. `StyleSheetParser` unifies qualified rules and
  /// at-rules under one type, so both yield `Option<StyleRule>`: `Some` for a
  /// style rule, `None` for an `@define-color` (whose payload is pushed onto
  /// `color_definitions` instead).
  struct SheetParser {
      color_definitions: Vec<(String, String)>,
  }

  impl<'i> QualifiedRuleParser<'i> for SheetParser {
      type Prelude = String;
      type QualifiedRule = Option<StyleRule>;
      type Error = ();

      fn parse_prelude<'t>(
          &mut self,
          input: &mut Parser<'i, 't>,
      ) -> Result<String, ParseError<'i, ()>> {
          Ok(remaining_text(input).trim().to_string())
      }

      fn parse_block<'t>(
          &mut self,
          prelude: String,
          _start: &ParserState,
          input: &mut Parser<'i, 't>,
      ) -> Result<Option<StyleRule>, ParseError<'i, ()>> {
          let mut block = DeclarationBlockParser;
          let mut declarations = Vec::new();
          for item in RuleBodyParser::<_, _, ()>::new(input, &mut block) {
              // A malformed declaration must not discard the rest of the
              // block -- GTK themes routinely carry properties this
              // milestone's tokenizer path cannot interpret.
              if let Ok(decl) = item {
                  declarations.push(decl);
              }
          }
          Ok(Some(StyleRule {
              selector_text: prelude,
              declarations,
              // Filled in by `parse_stylesheet`, which knows the index.
              source_order: 0,
          }))
      }
  }

  impl<'i> AtRuleParser<'i> for SheetParser {
      type Prelude = ();
      type AtRule = Option<StyleRule>;
      type Error = ();

      fn parse_prelude<'t>(
          &mut self,
          name: CowRcStr<'i>,
          input: &mut Parser<'i, 't>,
      ) -> Result<(), ParseError<'i, ()>> {
          if !name.eq_ignore_ascii_case("define-color") {
              return Err(input.new_custom_error(()));
          }
          let color_name = input.expect_ident()?.as_ref().to_string();
          let value = remaining_text(input).trim().to_string();
          if value.is_empty() {
              return Err(input.new_custom_error(()));
          }
          self.color_definitions.push((color_name, value));
          Ok(())
      }

      fn rule_without_block(
          &mut self,
          _prelude: (),
          _start: &ParserState,
      ) -> Result<Option<StyleRule>, ()> {
          Ok(None)
      }
  }

  /// Parse `css` into rules and `@define-color` pairs.
  ///
  /// Invalid rules are skipped rather than aborting the sheet, matching CSS's
  /// own error-recovery rules and GTK's tolerance of unknown syntax.
  #[must_use]
  pub fn parse_stylesheet(css: &str) -> Stylesheet {
      let mut input = ParserInput::new(css);
      let mut parser = Parser::new(&mut input);
      let mut sheet_parser = SheetParser {
          color_definitions: Vec::new(),
      };
      let mut rules: Vec<StyleRule> = Vec::new();
      for item in StyleSheetParser::new(&mut parser, &mut sheet_parser) {
          match item {
              Ok(Some(mut rule)) => {
                  rule.source_order = rules.len();
                  rules.push(rule);
              }
              Ok(None) => {}
              Err((err, slice)) => {
                  tracing::debug!(?err, rule = %slice.chars().take(80).collect::<String>(), "skipping invalid CSS rule");
              }
          }
      }
      Stylesheet {
          rules,
          color_definitions: sheet_parser.color_definitions,
      }
  }
  ```

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
  Expected: five tests pass (Task 1's plus the four here).

- [ ] **Commit:**
  ```bash
  git add ui/src/lib.rs ui/src/css
  git commit -m "$(cat <<'EOF'
  feat(ui): parse GTK CSS into rules and @define-color pairs

  cssparser 0.37 glue. Declaration values are kept as verbatim source text
  so GTK-only value syntax (image(), -gtk-*, relative color) survives
  parsing even where this milestone cannot interpret it. Invalid rules are
  skipped, not fatal: the whole 1,941-line Adwaita sheet parses.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 3 — `@define-color` table and color-value resolution

**Files**
- Create: `ui/src/css/colors.rs`
- Modify: `ui/src/css/mod.rs` (add `pub mod colors;`)
- Test: `ui/src/css/colors.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: `Stylesheet::color_definitions`.
- Produces:
  ```rust
  pub type ColorTable = std::collections::HashMap<String, Color>;   // skia_rs_safe::core::Color
  pub fn build_color_table(definitions: &[(String, String)]) -> ColorTable;
  pub fn parse_color_value(value: &str, table: &ColorTable) -> Option<Color>;
  ```

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/css/colors.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::{build_color_table, parse_color_value, ColorTable};
      use crate::css::parse::parse_stylesheet;
      use skia_rs_safe::core::Color;

      fn adwaita_table() -> ColorTable {
          build_color_table(&parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT).color_definitions)
      }

      #[test]
      fn resolves_adwaita_named_colors() {
          let table = adwaita_table();
          assert_eq!(table.get("theme_fg_color"), Some(&Color(0xFF2E_3436)));
          assert_eq!(table.get("borders"), Some(&Color(0xFFCD_C7C2)));
          assert_eq!(table.get("accent_color"), Some(&Color(0xFF35_84E4)));
          assert_eq!(table.get("theme_text_color"), Some(&Color(0xFF00_0000)));
          // rgba(255, 255, 255, 0.8) -> alpha 204 (0.8 * 255, rounded).
          assert_eq!(table.get("wm_highlight"), Some(&Color(0xCCFF_FFFF)));
      }

      #[test]
      fn relative_color_syntax_is_recorded_as_unresolved_not_fabricated() {
          let table = adwaita_table();
          // `@define-color wm_shadow rgb(from black r g b / calc(alpha * 0.35))`
          // is CSS Color 5 relative syntax, out of scope for M1.
          assert!(!table.contains_key("wm_shadow"));
          // 37 declared, 8 of them relative-color syntax.
          assert_eq!(table.len(), 29);
      }

      #[test]
      fn at_name_references_resolve_through_the_table() {
          let table = build_color_table(&[
              ("borders".to_string(), "#cdc7c2".to_string()),
              ("edge".to_string(), "@borders".to_string()),
          ]);
          assert_eq!(table.get("edge"), Some(&Color(0xFFCD_C7C2)));
          assert_eq!(parse_color_value("@edge", &table), Some(Color(0xFFCD_C7C2)));
          assert_eq!(parse_color_value("@nope", &table), None);
      }

      #[test]
      fn literal_forms_delegate_to_skias_css_parser() {
          let table = ColorTable::new();
          assert_eq!(parse_color_value("#2e3436", &table), Some(Color(0xFF2E_3436)));
          assert_eq!(parse_color_value("white", &table), Some(Color(0xFFFF_FFFF)));
          assert_eq!(parse_color_value("rgb(53, 132, 228)", &table), Some(Color(0xFF35_84E4)));
          assert_eq!(parse_color_value("transparent", &table), Some(Color(0x0000_0000)));
          assert_eq!(parse_color_value("linear-gradient(to top, red, blue)", &table), None);
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui css::colors
  ```
  Expected: `error[E0583]: file not found for module 'colors'` (or, once the file exists, `cannot find function 'build_color_table'`).

- [ ] **Write `ui/src/css/colors.rs`:**
  ```rust
  //! GTK's `@define-color` table and the color-value grammar M1 needs.
  //!
  //! Literal syntax (`#rgb`/`#rrggbb`, `rgb()`, `rgba()`, `hsl()`, CSS named
  //! colors) is delegated to `skia_rs_core::Color::from_css`, which already
  //! implements CSS Color Level 3 and 4. On top of that this module adds the
  //! one GTK extension the M1 slice needs: `@name` references into the
  //! `@define-color` table.
  //!
  //! Not implemented here (M2): GTK's `alpha()`, `shade()` and `mix()`
  //! functions, and CSS Color Level 5 relative syntax (`rgb(from ...)`,
  //! `hsl(from ...)`). No Adwaita `button` rule uses any of them; entries in
  //! the table whose value needs them are left unresolved rather than
  //! guessed at.

  use std::collections::HashMap;

  use skia_rs_safe::core::Color;

  /// Resolved `@define-color` names.
  pub type ColorTable = HashMap<String, Color>;

  /// Resolve one color value against `table`.
  ///
  /// Returns `None` for anything this milestone cannot interpret -- including
  /// values that are not colors at all (`linear-gradient(...)`), which is how
  /// `super::computed` distinguishes a flat background from a gradient.
  #[must_use]
  pub fn parse_color_value(value: &str, table: &ColorTable) -> Option<Color> {
      let value = value.trim();
      if let Some(name) = value.strip_prefix('@') {
          return table.get(name.trim()).copied();
      }
      Color::from_css(value)
  }

  /// Build the color table from `@define-color` pairs in source order.
  ///
  /// Resolution is incremental and order-sensitive, exactly as GTK's is: a
  /// definition may reference any name defined *above* it, and a later
  /// redefinition of a name wins for everything below it.
  #[must_use]
  pub fn build_color_table(definitions: &[(String, String)]) -> ColorTable {
      let mut table = ColorTable::new();
      for (name, value) in definitions {
          match parse_color_value(value, &table) {
              Some(color) => {
                  table.insert(name.clone(), color);
              }
              None => {
                  tracing::debug!(%name, %value, "unresolved @define-color; skipping");
              }
          }
      }
      table
  }
  ```

- [ ] **Add to `ui/src/css/mod.rs`:**
  ```rust
  pub mod colors;
  ```

- [ ] **Run the gates.** If `relative_color_syntax_is_recorded_as_unresolved_not_fabricated` reports a length other than 29, print the unresolved names and correct the constant to the real count before proceeding — the assertion exists to pin how much of Adwaita's palette M1 actually resolves:
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```

- [ ] **Commit:**
  ```bash
  git add ui/src/css
  git commit -m "$(cat <<'EOF'
  feat(ui): resolve @define-color into a color table

  Layers GTK's `@name` references over skia-rs-core's CSS Color 3/4 parser.
  Relative color syntax (8 of Adwaita's 37 definitions) is recorded as
  unresolved rather than fabricated; alpha()/shade()/mix() are M2 -- no
  Adwaita button rule needs them.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 4 — `selectors` integration: `GtkSelectorImpl` and `CssNode: Element`

**Files**
- Create: `ui/src/css/select.rs`
- Modify: `ui/src/css/mod.rs` (add `pub mod select;`)
- Test: `ui/src/css/select.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: nothing from earlier tasks (pure selector layer).
- Produces:
  ```rust
  pub struct GtkSelectorImpl;                                  // impl selectors::SelectorImpl
  pub struct CssString { /* text + precomputed hash */ }
  pub enum GtkPseudoClass { Hover, Active, Checked, Disabled, Focus, FocusVisible, Backdrop, Selected, Other(CssString) }
  pub struct GtkPseudoElement(pub CssString);
  pub struct GtkSelectorParser;                                // impl selectors::parser::Parser<'i>
  pub struct PseudoStates { pub hover: bool, pub active: bool, pub checked: bool, pub disabled: bool, pub focus: bool, pub backdrop: bool, pub selected: bool }
  pub struct CssNode(Rc<NodeData>);                            // impl selectors::Element
  impl CssNode {
      pub fn new(name: &str, classes: &[&str], states: PseudoStates, parent: Option<CssNode>) -> CssNode;
  }
  pub fn parse_selector_list(text: &str) -> Option<SelectorList<GtkSelectorImpl>>;
  pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &CssNode) -> bool;
  ```

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/css/select.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::{matches, parse_selector_list, CssNode, PseudoStates};

      fn window_button(classes: &[&str], states: PseudoStates) -> CssNode {
          let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
          CssNode::new("button", classes, states, Some(window))
      }

      fn hits(selector: &str, node: &CssNode) -> bool {
          let list = parse_selector_list(selector)
              .unwrap_or_else(|| panic!("`{selector}` failed to parse"));
          matches(&list, node)
      }

      #[test]
      fn element_name_and_child_combinator() {
          let node = window_button(&[], PseudoStates::default());
          assert!(hits("button", &node));
          assert!(hits("window > button", &node));
          assert!(hits("window button", &node));
          assert!(!hits("headerbar > button", &node));
          assert!(!hits("entry", &node));
      }

      #[test]
      fn pseudo_class_state_gates_matching() {
          let plain = window_button(&[], PseudoStates::default());
          let hovered = window_button(
              &[],
              PseudoStates {
                  hover: true,
                  ..PseudoStates::default()
              },
          );
          assert!(!hits("button:hover", &plain));
          assert!(hits("button:hover", &hovered));
          assert!(!hits("button:active", &hovered));
          assert!(hits("button", &hovered), "state must not break the base match");
      }

      #[test]
      fn style_classes_match() {
          let suggested = window_button(&["suggested-action"], PseudoStates::default());
          assert!(hits(".suggested-action", &suggested));
          assert!(hits("button.suggested-action", &suggested));
          assert!(!hits("button.destructive-action", &suggested));
          assert!(hits("window.background > button.suggested-action", &suggested));
      }

      #[test]
      fn unknown_pseudo_classes_parse_but_never_match() {
          // GTK carries pseudo-classes this milestone does not model. They must
          // not make the whole rule unparseable (which would silently drop
          // hundreds of Adwaita rules) and must not match either.
          let node = window_button(&[], PseudoStates::default());
          assert!(hits("button", &node));
          assert!(!hits("button:indeterminate", &node));
          assert!(parse_selector_list("button:indeterminate").is_some());
      }

      #[test]
      fn specificity_follows_css_rules() {
          let one = parse_selector_list("button").unwrap();
          let two = parse_selector_list("button:hover").unwrap();
          let three = parse_selector_list("window > button.suggested-action:hover").unwrap();
          let spec = |l: &selectors::SelectorList<super::GtkSelectorImpl>| l.slice()[0].specificity();
          assert!(spec(&one) < spec(&two));
          assert!(spec(&two) < spec(&three));
      }

      #[test]
      fn adwaitas_real_button_preludes_parse() {
          for prelude in [
              "notebook > header > tabs > arrow, button",
              "notebook > header > tabs > arrow:hover, button:hover",
              "button.suggested-action",
              "button.suggested-action:active, button.suggested-action:checked",
              "columnview.view > header > button, treeview.view > header > button",
          ] {
              assert!(
                  parse_selector_list(prelude).is_some(),
                  "Adwaita prelude failed to parse: {prelude}"
              );
          }
          let base = parse_selector_list("notebook > header > tabs > arrow, button").unwrap();
          assert_eq!(base.slice().len(), 2);
          let node = window_button(&[], PseudoStates::default());
          assert!(matches(&base, &node));
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui css::select
  ```
  Expected: `error[E0583]: file not found for module 'select'`.

- [ ] **Write `ui/src/css/select.rs`.** Note the shapes `selectors` 0.40 forces: `Element: Sized + Clone + Debug` with `parent_element(&self) -> Option<Self>`, so the node is an `Rc` handle; `Identifier`/`LocalName`/`NamespaceUrl` must all implement `PrecomputedHash`, so one `CssString` newtype carrying a cached FNV-1a hash serves every associated string type.
  ```rust
  //! GTK's CSS node model expressed through Servo's `selectors` crate.
  //!
  //! GTK widgets are not a DOM: nodes have an element name (`window`,
  //! `button`, `label`), style classes, pseudo-class state, and a parent --
  //! and nothing else. No ids, no attributes, no namespaces, no siblings that
  //! M1 needs. `CssNode` models exactly that and implements
  //! [`selectors::Element`], so the upstream matcher drives selection with no
  //! fork and no shim.

  use std::borrow::Borrow;
  use std::fmt;
  use std::rc::Rc;

  use cssparser::{CowRcStr, Parser as CssParser, ParserInput, SourceLocation, ToCss};
  use precomputed_hash::PrecomputedHash;
  use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
  use selectors::bloom::BloomFilter;
  use selectors::context::{
      MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
  };
  use selectors::matching::{matches_selector_list, ElementSelectorFlags, MatchingContext};
  use selectors::parser::{
      NonTSPseudoClass, ParseRelative, PseudoElement as PseudoElementTrait, SelectorParseErrorKind,
  };
  use selectors::{Element, OpaqueElement, SelectorImpl, SelectorList};

  /// An interned CSS identifier with a cached hash.
  ///
  /// `selectors` requires `PrecomputedHash` on its `Identifier`, `LocalName`
  /// and `NamespaceUrl` types; one newtype covers all of them.
  #[derive(Clone, Debug)]
  pub struct CssString {
      text: String,
      hash: u32,
  }

  /// FNV-1a, 32-bit. Cheap, stable, and adequate for the ancestor-hash
  /// filtering `selectors` uses it for.
  fn fnv1a(bytes: &[u8]) -> u32 {
      let mut hash: u32 = 0x811C_9DC5;
      for &b in bytes {
          hash ^= u32::from(b);
          hash = hash.wrapping_mul(0x0100_0193);
      }
      hash
  }

  impl CssString {
      /// Intern `text`, computing its hash once.
      #[must_use]
      pub fn new(text: &str) -> Self {
          Self {
              text: text.to_string(),
              hash: fnv1a(text.as_bytes()),
          }
      }

      /// The underlying text.
      #[must_use]
      pub fn as_str(&self) -> &str {
          &self.text
      }
  }

  impl PartialEq for CssString {
      fn eq(&self, other: &Self) -> bool {
          self.text == other.text
      }
  }

  impl Eq for CssString {}

  impl Default for CssString {
      fn default() -> Self {
          Self::new("")
      }
  }

  impl From<&str> for CssString {
      fn from(value: &str) -> Self {
          Self::new(value)
      }
  }

  impl Borrow<str> for CssString {
      fn borrow(&self) -> &str {
          &self.text
      }
  }

  impl PrecomputedHash for CssString {
      fn precomputed_hash(&self) -> u32 {
          self.hash
      }
  }

  impl ToCss for CssString {
      fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
          dest.write_str(&self.text)
      }
  }

  /// The pseudo-classes M1 models, plus a catch-all.
  ///
  /// `Other` exists so a rule carrying a pseudo-class this milestone does not
  /// model still *parses* -- dropping it would silently discard large parts of
  /// a real theme. `Other` never matches.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum GtkPseudoClass {
      Hover,
      Active,
      Checked,
      Disabled,
      Focus,
      FocusVisible,
      Backdrop,
      Selected,
      Other(CssString),
  }

  impl ToCss for GtkPseudoClass {
      fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
          dest.write_char(':')?;
          match self {
              Self::Hover => dest.write_str("hover"),
              Self::Active => dest.write_str("active"),
              Self::Checked => dest.write_str("checked"),
              Self::Disabled => dest.write_str("disabled"),
              Self::Focus => dest.write_str("focus"),
              Self::FocusVisible => dest.write_str("focus-visible"),
              Self::Backdrop => dest.write_str("backdrop"),
              Self::Selected => dest.write_str("selected"),
              Self::Other(name) => dest.write_str(name.as_str()),
          }
      }
  }

  impl NonTSPseudoClass for GtkPseudoClass {
      type Impl = GtkSelectorImpl;

      fn is_active_or_hover(&self) -> bool {
          matches!(self, Self::Hover | Self::Active)
      }

      fn is_user_action_state(&self) -> bool {
          matches!(self, Self::Hover | Self::Active | Self::Focus | Self::FocusVisible)
      }
  }

  /// A pseudo-element (`::selection`, ...). Parsed so rules using them are not
  /// discarded; never matched, because M1 has no pseudo-element boxes.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct GtkPseudoElement(pub CssString);

  impl ToCss for GtkPseudoElement {
      fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
          dest.write_str("::")?;
          dest.write_str(self.0.as_str())
      }
  }

  impl PseudoElementTrait for GtkPseudoElement {
      type Impl = GtkSelectorImpl;
  }

  /// The `SelectorImpl` binding GTK's node model to `selectors`.
  #[derive(Clone, Debug)]
  pub struct GtkSelectorImpl;

  impl SelectorImpl for GtkSelectorImpl {
      type ExtraMatchingData<'a> = ();
      type AttrValue = CssString;
      type Identifier = CssString;
      type LocalName = CssString;
      type NamespaceUrl = CssString;
      type NamespacePrefix = CssString;
      type BorrowedNamespaceUrl = str;
      type BorrowedLocalName = str;
      type NonTSPseudoClass = GtkPseudoClass;
      type PseudoElement = GtkPseudoElement;
  }

  /// The selector-syntax parser: maps `:name` onto [`GtkPseudoClass`].
  pub struct GtkSelectorParser;

  impl<'i> selectors::parser::Parser<'i> for GtkSelectorParser {
      type Impl = GtkSelectorImpl;
      type Error = SelectorParseErrorKind<'i>;

      fn parse_is_and_where(&self) -> bool {
          true
      }

      fn parse_non_ts_pseudo_class(
          &self,
          _location: SourceLocation,
          name: CowRcStr<'i>,
      ) -> Result<GtkPseudoClass, cssparser::ParseError<'i, Self::Error>> {
          Ok(match_pseudo_class_name(name.as_ref()))
      }

      fn parse_pseudo_element(
          &self,
          _location: SourceLocation,
          name: CowRcStr<'i>,
      ) -> Result<GtkPseudoElement, cssparser::ParseError<'i, Self::Error>> {
          Ok(GtkPseudoElement(CssString::new(name.as_ref())))
      }
  }

  fn match_pseudo_class_name(name: &str) -> GtkPseudoClass {
      match name.to_ascii_lowercase().as_str() {
          "hover" => GtkPseudoClass::Hover,
          "active" => GtkPseudoClass::Active,
          "checked" => GtkPseudoClass::Checked,
          "disabled" => GtkPseudoClass::Disabled,
          "focus" => GtkPseudoClass::Focus,
          "focus-visible" => GtkPseudoClass::FocusVisible,
          "backdrop" => GtkPseudoClass::Backdrop,
          "selected" => GtkPseudoClass::Selected,
          other => GtkPseudoClass::Other(CssString::new(other)),
      }
  }

  /// The pseudo-class state a node currently carries.
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct PseudoStates {
      pub hover: bool,
      pub active: bool,
      pub checked: bool,
      pub disabled: bool,
      pub focus: bool,
      pub backdrop: bool,
      pub selected: bool,
  }

  /// The immutable payload behind a [`CssNode`].
  #[derive(Debug)]
  pub struct NodeData {
      name: CssString,
      classes: Vec<CssString>,
      states: PseudoStates,
      parent: Option<CssNode>,
  }

  /// A GTK CSS node: element name, style classes, pseudo-class state, parent.
  ///
  /// Immutable and `Rc`-shared, because `Element::parent_element` must return
  /// `Self` by value. Changing state means rebuilding the node (cheap: the
  /// ancestors are shared), which is also exactly what a restyle is.
  #[derive(Clone, Debug)]
  pub struct CssNode(Rc<NodeData>);

  impl CssNode {
      /// Build a node with the given identity, state and parent.
      #[must_use]
      pub fn new(
          name: &str,
          classes: &[&str],
          states: PseudoStates,
          parent: Option<CssNode>,
      ) -> Self {
          Self(Rc::new(NodeData {
              name: CssString::new(name),
              classes: classes.iter().map(|c| CssString::new(c)).collect(),
              states,
              parent,
          }))
      }

      /// A copy of this node with different pseudo-class state, sharing the
      /// same name, classes and ancestors.
      #[must_use]
      pub fn with_states(&self, states: PseudoStates) -> Self {
          Self(Rc::new(NodeData {
              name: self.0.name.clone(),
              classes: self.0.classes.clone(),
              states,
              parent: self.0.parent.clone(),
          }))
      }

      /// This node's pseudo-class state.
      #[must_use]
      pub fn states(&self) -> PseudoStates {
          self.0.states
      }
  }

  impl Element for CssNode {
      type Impl = GtkSelectorImpl;

      fn opaque(&self) -> OpaqueElement {
          OpaqueElement::new(Rc::as_ptr(&self.0))
      }

      fn parent_element(&self) -> Option<Self> {
          self.0.parent.clone()
      }

      fn parent_node_is_shadow_root(&self) -> bool {
          false
      }

      fn containing_shadow_host(&self) -> Option<Self> {
          None
      }

      fn is_pseudo_element(&self) -> bool {
          false
      }

      fn prev_sibling_element(&self) -> Option<Self> {
          None
      }

      fn next_sibling_element(&self) -> Option<Self> {
          None
      }

      fn first_element_child(&self) -> Option<Self> {
          None
      }

      fn is_html_element_in_html_document(&self) -> bool {
          false
      }

      fn has_local_name(&self, local_name: &str) -> bool {
          self.0.name.as_str() == local_name
      }

      fn has_namespace(&self, ns: &str) -> bool {
          ns.is_empty()
      }

      fn is_same_type(&self, other: &Self) -> bool {
          self.0.name == other.0.name
      }

      fn attr_matches(
          &self,
          _ns: &NamespaceConstraint<&CssString>,
          _local_name: &CssString,
          _operation: &AttrSelectorOperation<&CssString>,
      ) -> bool {
          false
      }

      fn match_non_ts_pseudo_class(
          &self,
          pc: &GtkPseudoClass,
          _context: &mut MatchingContext<GtkSelectorImpl>,
      ) -> bool {
          let s = self.0.states;
          match pc {
              GtkPseudoClass::Hover => s.hover,
              GtkPseudoClass::Active => s.active,
              GtkPseudoClass::Checked => s.checked,
              GtkPseudoClass::Disabled => s.disabled,
              GtkPseudoClass::Focus | GtkPseudoClass::FocusVisible => s.focus,
              GtkPseudoClass::Backdrop => s.backdrop,
              GtkPseudoClass::Selected => s.selected,
              GtkPseudoClass::Other(_) => false,
          }
      }

      fn match_pseudo_element(
          &self,
          _pe: &GtkPseudoElement,
          _context: &mut MatchingContext<GtkSelectorImpl>,
      ) -> bool {
          false
      }

      fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

      fn is_link(&self) -> bool {
          false
      }

      fn is_html_slot_element(&self) -> bool {
          false
      }

      fn has_id(&self, _id: &CssString, _case_sensitivity: CaseSensitivity) -> bool {
          false
      }

      fn has_class(&self, name: &CssString, _case_sensitivity: CaseSensitivity) -> bool {
          self.0.classes.iter().any(|c| c == name)
      }

      fn has_custom_state(&self, _name: &CssString) -> bool {
          false
      }

      fn imported_part(&self, _name: &CssString) -> Option<CssString> {
          None
      }

      fn is_part(&self, _name: &CssString) -> bool {
          false
      }

      fn is_empty(&self) -> bool {
          true
      }

      fn is_root(&self) -> bool {
          self.0.parent.is_none()
      }

      fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
          false
      }
  }

  /// Parse a comma-separated selector list. `None` if the whole list is invalid.
  #[must_use]
  pub fn parse_selector_list(text: &str) -> Option<SelectorList<GtkSelectorImpl>> {
      let mut input = ParserInput::new(text);
      let mut parser = CssParser::new(&mut input);
      SelectorList::parse(&GtkSelectorParser, &mut parser, ParseRelative::No).ok()
  }

  /// Whether any selector in `list` matches `node`.
  #[must_use]
  pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &CssNode) -> bool {
      let mut caches = SelectorCaches::default();
      let mut context = MatchingContext::new(
          MatchingMode::Normal,
          None,
          &mut caches,
          QuirksMode::NoQuirks,
          NeedsSelectorFlags::No,
          MatchingForInvalidation::No,
      );
      matches_selector_list(list, node, &mut context)
  }
  ```

- [ ] **Add to `ui/src/css/mod.rs`:** `pub mod select;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
  Expected: the six new tests pass. This answers the spec's risk (a) — Servo's matcher adapts to GTK's node model with no fork.

- [ ] **Commit:**
  ```bash
  git add ui/src/css
  git commit -m "$(cat <<'EOF'
  feat(ui): match GTK CSS nodes with Servo's selectors crate

  GtkSelectorImpl + a CssNode (name, classes, pseudo-state, parent) that
  implements selectors::Element, so `window > button`, `button:hover` and
  `.suggested-action` all match through the upstream matcher unmodified --
  resolving the spec's risk (a). Unmodelled pseudo-classes parse into a
  never-matching `Other` variant rather than making whole rules
  unparseable.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 5 — Cascade: matched declarations by specificity and source order

**Files**
- Create: `ui/src/css/cascade.rs`
- Modify: `ui/src/css/mod.rs` (add `pub mod cascade;`)
- Test: `ui/src/css/cascade.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: `parse::{Stylesheet, StyleRule, Declaration}`, `colors::{ColorTable, build_color_table}`, `select::{CssNode, GtkSelectorImpl, parse_selector_list, matches}`.
- Produces:
  ```rust
  pub struct CompiledRule { pub selectors: SelectorList<GtkSelectorImpl>, pub declarations: Vec<Declaration>, pub source_order: usize }
  pub struct CompiledSheet { pub rules: Vec<CompiledRule>, pub colors: ColorTable }
  impl CompiledSheet { pub fn compile(css: &str) -> CompiledSheet; }
  pub fn cascade(sheet: &CompiledSheet, node: &CssNode) -> HashMap<String, String>;
  ```

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/css/cascade.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::{cascade, CompiledSheet};
      use crate::css::select::{CssNode, PseudoStates};

      fn button(classes: &[&str], states: PseudoStates) -> CssNode {
          let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
          CssNode::new("button", classes, states, Some(window))
      }

      #[test]
      fn higher_specificity_wins_over_source_order() {
          let sheet = CompiledSheet::compile(
              "button.suggested-action { color: blue }\n\
               button { color: red }",
          );
          let node = button(&["suggested-action"], PseudoStates::default());
          assert_eq!(cascade(&sheet, &node).get("color").map(String::as_str), Some("blue"));
      }

      #[test]
      fn equal_specificity_falls_back_to_source_order() {
          let sheet = CompiledSheet::compile("button { color: red }\nbutton { color: green }");
          let node = button(&[], PseudoStates::default());
          assert_eq!(cascade(&sheet, &node).get("color").map(String::as_str), Some("green"));
      }

      #[test]
      fn important_beats_specificity() {
          let sheet = CompiledSheet::compile(
              "button { color: red !important }\n\
               window > button.suggested-action { color: blue }",
          );
          let node = button(&["suggested-action"], PseudoStates::default());
          assert_eq!(cascade(&sheet, &node).get("color").map(String::as_str), Some("red"));
      }

      #[test]
      fn non_matching_rules_contribute_nothing() {
          let sheet = CompiledSheet::compile("entry { color: red }\nbutton:hover { color: blue }");
          let node = button(&[], PseudoStates::default());
          assert!(cascade(&sheet, &node).is_empty());
      }

      #[test]
      fn adwaita_button_cascade_resolves_the_expected_declarations() {
          let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
          let normal = cascade(&sheet, &button(&[], PseudoStates::default()));
          assert_eq!(normal.get("border-radius").map(String::as_str), Some("5px"));
          assert_eq!(normal.get("padding").map(String::as_str), Some("4px 9px"));
          assert_eq!(normal.get("color").map(String::as_str), Some("#2e3436"));
          assert_eq!(normal.get("border-color").map(String::as_str), Some("#cdc7c2"));
          assert_eq!(
              normal.get("background-image").map(String::as_str),
              Some("linear-gradient(to top, #f6f5f4 2px, #fbfafa)")
          );

          let hovered = cascade(
              &sheet,
              &button(&[], PseudoStates { hover: true, ..PseudoStates::default() }),
          );
          assert_eq!(
              hovered.get("background-image").map(String::as_str),
              Some("linear-gradient(to top, #d6d1cd, #e8e6e3 1px)")
          );

          let active = cascade(
              &sheet,
              &button(&[], PseudoStates { active: true, ..PseudoStates::default() }),
          );
          assert_eq!(active.get("background-image").map(String::as_str), Some("image(#dad6d2)"));

          let suggested = cascade(&sheet, &button(&["suggested-action"], PseudoStates::default()));
          assert_eq!(suggested.get("color").map(String::as_str), Some("white"));
          assert_eq!(suggested.get("border-color").map(String::as_str), Some("#15539e"));
          assert_eq!(
              suggested.get("background-image").map(String::as_str),
              Some("linear-gradient(to top, #2c7fe3 2px, #3584e4)")
          );
      }

      #[test]
      fn compile_carries_the_color_table() {
          let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
          assert_eq!(
              sheet.colors.get("accent_color"),
              Some(&skia_rs_safe::core::Color(0xFF35_84E4))
          );
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui css::cascade
  ```
  Expected: `error[E0583]: file not found for module 'cascade'`.

- [ ] **Write `ui/src/css/cascade.rs`:**
  ```rust
  //! The cascade: which declarations a node actually gets.
  //!
  //! Origin is not modelled (M1 loads exactly one author sheet), so the sort
  //! key is CSS's remainder: `!important` first, then selector specificity,
  //! then source order. Later wins on a tie, which is what makes a theme's
  //! own overrides work.

  use std::collections::HashMap;

  use selectors::SelectorList;

  use super::colors::{build_color_table, ColorTable};
  use super::parse::{parse_stylesheet, Declaration};
  use super::select::{matches, parse_selector_list, CssNode, GtkSelectorImpl};

  /// A rule whose prelude has been compiled to a selector list.
  #[derive(Debug)]
  pub struct CompiledRule {
      /// The rule's selectors.
      pub selectors: SelectorList<GtkSelectorImpl>,
      /// The rule's declarations.
      pub declarations: Vec<Declaration>,
      /// Index of this rule in the source sheet.
      pub source_order: usize,
  }

  /// A stylesheet ready for matching.
  #[derive(Debug)]
  pub struct CompiledSheet {
      /// Rules whose preludes parsed.
      pub rules: Vec<CompiledRule>,
      /// Resolved `@define-color` names.
      pub colors: ColorTable,
  }

  impl CompiledSheet {
      /// Parse and compile `css`.
      ///
      /// Rules whose prelude does not parse are dropped with a debug log;
      /// a real theme's unparseable corners must not take the sheet with them.
      #[must_use]
      pub fn compile(css: &str) -> Self {
          let sheet = parse_stylesheet(css);
          let colors = build_color_table(&sheet.color_definitions);
          let mut rules = Vec::with_capacity(sheet.rules.len());
          for rule in sheet.rules {
              match parse_selector_list(&rule.selector_text) {
                  Some(selectors) => rules.push(CompiledRule {
                      selectors,
                      declarations: rule.declarations,
                      source_order: rule.source_order,
                  }),
                  None => {
                      tracing::debug!(prelude = %rule.selector_text, "unparseable selector list; dropping rule");
                  }
              }
          }
          Self { rules, colors }
      }
  }

  /// The declarations `node` wins, keyed by property name.
  ///
  /// Specificity is taken per *selector*, not per rule: a rule whose prelude
  /// is `notebook > header > tabs > arrow, button` contributes the specificity
  /// of whichever of its selectors actually matched.
  #[must_use]
  pub fn cascade(sheet: &CompiledSheet, node: &CssNode) -> HashMap<String, String> {
      // (important, specificity, source order) -> value, per property.
      let mut winners: HashMap<String, (bool, u32, usize, String)> = HashMap::new();

      for rule in &sheet.rules {
          let Some(specificity) = rule
              .selectors
              .slice()
              .iter()
              .filter(|selector| {
                  matches(&SelectorList::from_one((*selector).clone()), node)
              })
              .map(selectors::parser::Selector::specificity)
              .max()
          else {
              continue;
          };

          for decl in &rule.declarations {
              let key = (decl.important, specificity, rule.source_order);
              match winners.get(&decl.name) {
                  Some((imp, spec, order, _)) if (*imp, *spec, *order) >= key => {}
                  _ => {
                      winners.insert(
                          decl.name.clone(),
                          (key.0, key.1, key.2, decl.value.clone()),
                      );
                  }
              }
          }
      }

      winners
          .into_iter()
          .map(|(name, (_, _, _, value))| (name, value))
          .collect()
  }
  ```

- [ ] **Add to `ui/src/css/mod.rs`:** `pub mod cascade;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```

- [ ] **Commit:**
  ```bash
  git add ui/src/css
  git commit -m "$(cat <<'EOF'
  feat(ui): cascade matched declarations by specificity and order

  Compiles each rule's prelude to a SelectorList once, then orders matched
  declarations by (!important, specificity, source order). Specificity is
  taken per matched selector, not per rule, so Adwaita's shared preludes
  (`notebook > header > tabs > arrow, button`) score correctly.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 6 — `ComputedStyle`: the properties the slice paints

**Files**
- Create: `ui/src/css/computed.rs`
- Modify: `ui/src/css/mod.rs` (add `pub mod computed;`)
- Test: `ui/src/css/computed.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: `cascade::{CompiledSheet, cascade}`, `colors::{ColorTable, parse_color_value}`, `select::CssNode`.
- Produces:
  ```rust
  pub struct GradientStop { pub color: Color, pub position_px: Option<f32> }
  pub enum Background { Transparent, Solid(Color), LinearGradientToTop { from: GradientStop, to: GradientStop } }
  impl Background { pub fn color_at(&self, height: f32, y_from_top: f32) -> Color; }
  pub struct ComputedStyle {
      pub background: Background, pub color: Color,
      pub border_width: f32, pub border_color: Color, pub border_radius: f32,
      pub padding: [f32; 4],            // top, right, bottom, left
      pub min_width: f32, pub min_height: f32, pub font_size: f32,
  }
  impl ComputedStyle {
      pub const DEFAULT_FONT_SIZE: f32 = 14.0;
      pub fn resolve(sheet: &CompiledSheet, node: &CssNode) -> ComputedStyle;
  }
  ```
  `color_at` takes `y_from_top` because that is the coordinate the painter has; `to top` means the gradient line runs bottom→top, so it converts internally.

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/css/computed.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::{Background, ComputedStyle, GradientStop};
      use crate::css::cascade::CompiledSheet;
      use crate::css::select::{CssNode, PseudoStates};
      use skia_rs_safe::core::Color;

      fn adwaita() -> CompiledSheet {
          CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT)
      }

      fn button(classes: &[&str], states: PseudoStates) -> CssNode {
          let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
          CssNode::new("button", classes, states, Some(window))
      }

      fn stop(color: u32, position_px: Option<f32>) -> GradientStop {
          GradientStop { color: Color(color), position_px }
      }

      #[test]
      fn adwaita_normal_button() {
          let s = ComputedStyle::resolve(&adwaita(), &button(&[], PseudoStates::default()));
          assert_eq!(
              s.background,
              Background::LinearGradientToTop {
                  from: stop(0xFFF6_F5F4, Some(2.0)),
                  to: stop(0xFFFB_FAFA, None),
              }
          );
          assert_eq!(s.color, Color(0xFF2E_3436));
          assert_eq!(s.border_width, 1.0);
          assert_eq!(s.border_color, Color(0xFFCD_C7C2));
          assert_eq!(s.border_radius, 5.0);
          assert_eq!(s.padding, [4.0, 9.0, 4.0, 9.0]);
          assert_eq!(s.min_width, 16.0);
          assert_eq!(s.min_height, 24.0);
          assert_eq!(s.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
      }

      #[test]
      fn adwaita_hover_and_active_button() {
          let sheet = adwaita();
          let hovered = ComputedStyle::resolve(
              &sheet,
              &button(&[], PseudoStates { hover: true, ..PseudoStates::default() }),
          );
          assert_eq!(
              hovered.background,
              Background::LinearGradientToTop {
                  from: stop(0xFFD6_D1CD, None),
                  to: stop(0xFFE8_E6E3, Some(1.0)),
              }
          );
          assert_eq!(hovered.border_color, Color(0xFFCD_C7C2));

          let active = ComputedStyle::resolve(
              &sheet,
              &button(&[], PseudoStates { active: true, ..PseudoStates::default() }),
          );
          assert_eq!(active.background, Background::Solid(Color(0xFFDA_D6D2)));
          assert_eq!(active.border_radius, 5.0, "radius must survive the state change");
      }

      #[test]
      fn adwaita_suggested_action_button() {
          let sheet = adwaita();
          let s = ComputedStyle::resolve(&sheet, &button(&["suggested-action"], PseudoStates::default()));
          assert_eq!(
              s.background,
              Background::LinearGradientToTop {
                  from: stop(0xFF2C_7FE3, Some(2.0)),
                  to: stop(0xFF35_84E4, None),
              }
          );
          assert_eq!(s.color, Color(0xFFFF_FFFF));
          assert_eq!(s.border_color, Color(0xFF15_539E));

          let pressed = ComputedStyle::resolve(
              &sheet,
              &button(&["suggested-action"], PseudoStates { active: true, ..PseudoStates::default() }),
          );
          assert_eq!(pressed.background, Background::Solid(Color(0xFF19_61B9)));
      }

      #[test]
      fn gradient_sampling_follows_to_top_semantics() {
          // to top => the gradient line starts at the bottom edge.
          let hover = Background::LinearGradientToTop {
              from: stop(0xFFD6_D1CD, None),
              to: stop(0xFFE8_E6E3, Some(1.0)),
          };
          let h = 30.0;
          // Bottom row is the first stop.
          assert_eq!(hover.color_at(h, h - 0.5), Color(0xFFD6_D1CD));
          // Anything above 1px from the bottom is past the second stop.
          assert_eq!(hover.color_at(h, h / 2.0), Color(0xFFE8_E6E3));
          assert_eq!(hover.color_at(h, 0.5), Color(0xFFE8_E6E3));

          let normal = Background::LinearGradientToTop {
              from: stop(0xFFF6_F5F4, Some(2.0)),
              to: stop(0xFFFB_FAFA, None),
          };
          // Below the 2px first stop the fill is flat.
          assert_eq!(normal.color_at(h, h - 0.5), Color(0xFFF6_F5F4));
          assert_eq!(normal.color_at(h, h - 1.5), Color(0xFFF6_F5F4));
          // The top edge is the second stop.
          assert_eq!(normal.color_at(h, 0.0), Color(0xFFFB_FAFA));
          // The middle is strictly between the two.
          let mid = normal.color_at(h, h / 2.0);
          assert!(mid != Color(0xFFF6_F5F4) && mid != Color(0xFFFB_FAFA));
          assert!((0xF6..=0xFB).contains(&mid.red()));
          assert!((0xF5..=0xFA).contains(&mid.green()));
          assert!((0xF4..=0xFA).contains(&mid.blue()));
      }

      #[test]
      fn background_color_applies_when_there_is_no_background_image() {
          let sheet = CompiledSheet::compile("button { background-color: #112233 }");
          let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
          assert_eq!(s.background, Background::Solid(Color(0xFF11_2233)));
      }

      #[test]
      fn background_image_none_falls_back_to_background_color() {
          let sheet = CompiledSheet::compile(
              "button { background-color: #112233; background-image: none }",
          );
          let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
          assert_eq!(s.background, Background::Solid(Color(0xFF11_2233)));
      }

      #[test]
      fn named_color_references_resolve_through_define_color() {
          let sheet = CompiledSheet::compile(
              "@define-color accent_color #3584E4;\nbutton { background-color: @accent_color }",
          );
          let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
          assert_eq!(s.background, Background::Solid(Color(0xFF35_84E4)));
      }

      #[test]
      fn defaults_apply_when_nothing_matches() {
          let sheet = CompiledSheet::compile("entry { color: red }");
          let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
          assert_eq!(s.background, Background::Transparent);
          assert_eq!(s.color, Color::BLACK);
          assert_eq!(s.border_width, 0.0);
          assert_eq!(s.border_radius, 0.0);
          assert_eq!(s.padding, [0.0; 4]);
          assert_eq!(s.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui css::computed
  ```
  Expected: `error[E0583]: file not found for module 'computed'`.

- [ ] **Write `ui/src/css/computed.rs`:**
  ```rust
  //! Computed values for the properties the M1 slice paints.
  //!
  //! Deliberately narrow: `background-color`, `background-image`, `color`,
  //! `border`/`border-width`/`border-color`/`border-radius`, `padding`,
  //! `min-width`, `min-height`, `font-size`. Everything else in a real theme
  //! is parsed (Task 2) and cascaded (Task 5) but not yet interpreted.
  //!
  //! `background` is an enum rather than a `Color` because Adwaita's buttons
  //! have no `background-color` at all -- every button background is a
  //! `background-image`, either GTK's `image(<color>)` flat-fill extension or
  //! a two-stop `linear-gradient(to top, ...)`.

  use std::collections::HashMap;

  use skia_rs_safe::core::Color;

  use super::cascade::{cascade, CompiledSheet};
  use super::colors::{parse_color_value, ColorTable};
  use super::select::CssNode;

  /// One gradient stop: a color and an optional absolute position, measured in
  /// pixels along the gradient line from its origin.
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct GradientStop {
      /// The stop's color.
      pub color: Color,
      /// Distance in px from the gradient line's origin, or `None` for the
      /// implicit position (0 for the first stop, the full length for the last).
      pub position_px: Option<f32>,
  }

  /// A resolved background.
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub enum Background {
      /// Nothing painted.
      Transparent,
      /// A flat fill -- `background-color`, or GTK's `image(<color>)`.
      Solid(Color),
      /// `linear-gradient(to top, <stop>, <stop>)`. The gradient line runs
      /// from the box's bottom edge (`from`) to its top edge (`to`).
      LinearGradientToTop {
          /// The stop at the gradient line's origin (the bottom edge).
          from: GradientStop,
          /// The stop at the gradient line's end (the top edge).
          to: GradientStop,
      },
  }

  /// Round-to-nearest linear interpolation between two bytes.
  fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
      let a = f32::from(a);
      let b = f32::from(b);
      (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
  }

  impl Background {
      /// The color this background paints at `y_from_top` in a box `height` tall.
      ///
      /// `y_from_top` is the painter's coordinate; `to top` gradients are
      /// sampled along a line whose origin is the *bottom* edge, so this
      /// converts. Positions before the first stop and after the last clamp to
      /// that stop's color, per CSS's gradient-stop rules.
      #[must_use]
      pub fn color_at(&self, height: f32, y_from_top: f32) -> Color {
          match self {
              Self::Transparent => Color::TRANSPARENT,
              Self::Solid(color) => *color,
              Self::LinearGradientToTop { from, to } => {
                  let line = height.max(1.0);
                  let p0 = from.position_px.unwrap_or(0.0);
                  let p1 = to.position_px.unwrap_or(line);
                  let y = (line - y_from_top).clamp(0.0, line);
                  if y <= p0 || (p1 - p0).abs() < f32::EPSILON {
                      return from.color;
                  }
                  if y >= p1 {
                      return to.color;
                  }
                  let t = (y - p0) / (p1 - p0);
                  Color::from_argb(
                      lerp_u8(from.color.alpha(), to.color.alpha(), t),
                      lerp_u8(from.color.red(), to.color.red(), t),
                      lerp_u8(from.color.green(), to.color.green(), t),
                      lerp_u8(from.color.blue(), to.color.blue(), t),
                  )
              }
          }
      }
  }

  /// The properties the M1 slice needs, fully resolved.
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct ComputedStyle {
      /// Resolved background.
      pub background: Background,
      /// Foreground (text) color.
      pub color: Color,
      /// Uniform border width in px.
      pub border_width: f32,
      /// Border color.
      pub border_color: Color,
      /// Uniform corner radius in px.
      pub border_radius: f32,
      /// `[top, right, bottom, left]` in px.
      pub padding: [f32; 4],
      /// `min-width` in px.
      pub min_width: f32,
      /// `min-height` in px.
      pub min_height: f32,
      /// Font size in px.
      pub font_size: f32,
  }

  impl ComputedStyle {
      /// The font size used when no rule sets one.
      ///
      /// No Adwaita `button` rule declares `font-size`; GTK inherits it from
      /// the desktop's font setting, which M1 does not read.
      pub const DEFAULT_FONT_SIZE: f32 = 14.0;
  }

  impl Default for ComputedStyle {
      fn default() -> Self {
          Self {
              background: Background::Transparent,
              color: Color::BLACK,
              border_width: 0.0,
              border_color: Color::TRANSPARENT,
              border_radius: 0.0,
              padding: [0.0; 4],
              min_width: 0.0,
              min_height: 0.0,
              font_size: Self::DEFAULT_FONT_SIZE,
          }
      }
  }

  /// Parse a `<length>` in px. Unitless `0` is accepted; other units are not
  /// (M1 has no font/viewport context to resolve `em`/`%` against).
  fn parse_px(value: &str) -> Option<f32> {
      let value = value.trim();
      if let Some(number) = value.strip_suffix("px") {
          return number.trim().parse::<f32>().ok();
      }
      match value.parse::<f32>() {
          Ok(n) if n == 0.0 => Some(0.0),
          _ => None,
      }
  }

  /// Split a function call `name(args)` into `(name, args)`.
  fn split_function(value: &str) -> Option<(&str, &str)> {
      let value = value.trim();
      let open = value.find('(')?;
      let inner = value.strip_suffix(')')?;
      Some((value[..open].trim(), &inner[open + 1..]))
  }

  /// Split `args` on top-level commas (parenthesised commas stay together).
  fn split_top_level_commas(args: &str) -> Vec<String> {
      let mut parts = Vec::new();
      let mut depth = 0usize;
      let mut current = String::new();
      for ch in args.chars() {
          match ch {
              '(' => {
                  depth += 1;
                  current.push(ch);
              }
              ')' => {
                  depth = depth.saturating_sub(1);
                  current.push(ch);
              }
              ',' if depth == 0 => {
                  parts.push(current.trim().to_string());
                  current = String::new();
              }
              _ => current.push(ch),
          }
      }
      if !current.trim().is_empty() {
          parts.push(current.trim().to_string());
      }
      parts
  }

  /// Parse one `<color> [<length>]` gradient stop.
  fn parse_stop(text: &str, colors: &ColorTable) -> Option<GradientStop> {
      let text = text.trim();
      if let Some((color_text, position_text)) = text.rsplit_once(char::is_whitespace) {
          if let Some(position) = parse_px(position_text) {
              return Some(GradientStop {
                  color: parse_color_value(color_text, colors)?,
                  position_px: Some(position),
              });
          }
      }
      Some(GradientStop {
          color: parse_color_value(text, colors)?,
          position_px: None,
      })
  }

  /// Parse a `background-image` value into a [`Background`].
  ///
  /// Supports GTK's `image(<color>)` flat fill and the two-stop
  /// `linear-gradient(to top, ...)` form Adwaita's buttons use. Anything else
  /// -- radial gradients, `url()`, `-gtk-*` image functions, more than two
  /// stops -- yields `None`, which leaves the `background-color` value (or the
  /// transparent default) in place rather than painting something invented.
  fn parse_background_image(value: &str, colors: &ColorTable) -> Option<Background> {
      let (name, args) = split_function(value)?;
      match name {
          "image" => Some(Background::Solid(parse_color_value(args, colors)?)),
          "linear-gradient" => {
              let parts = split_top_level_commas(args);
              let [direction, first, second] = parts.as_slice() else {
                  return None;
              };
              if direction.trim() != "to top" {
                  return None;
              }
              Some(Background::LinearGradientToTop {
                  from: parse_stop(first, colors)?,
                  to: parse_stop(second, colors)?,
              })
          }
          _ => None,
      }
  }

  /// Parse a CSS 1-to-4-value `padding` shorthand into `[top, right, bottom, left]`.
  fn parse_padding(value: &str) -> Option<[f32; 4]> {
      let parts: Vec<f32> = value
          .split_whitespace()
          .map(parse_px)
          .collect::<Option<Vec<f32>>>()?;
      Some(match parts.as_slice() {
          [all] => [*all; 4],
          [tb, lr] => [*tb, *lr, *tb, *lr],
          [t, lr, b] => [*t, *lr, *b, *lr],
          [t, r, b, l] => [*t, *r, *b, *l],
          _ => return None,
      })
  }

  impl ComputedStyle {
      /// Resolve `node`'s style against `sheet`.
      #[must_use]
      pub fn resolve(sheet: &CompiledSheet, node: &CssNode) -> Self {
          Self::from_declarations(&cascade(sheet, node), &sheet.colors)
      }

      /// Resolve a set of winning declarations. Separated from [`Self::resolve`]
      /// so the property logic is testable without a node tree.
      #[must_use]
      pub fn from_declarations(
          declarations: &HashMap<String, String>,
          colors: &ColorTable,
      ) -> Self {
          let mut style = Self::default();
          let get = |name: &str| declarations.get(name).map(String::as_str);

          if let Some(color) = get("color").and_then(|v| parse_color_value(v, colors)) {
              style.color = color;
          }

          // `background-color` first, then `background-image` on top: CSS
          // paints the image over the color, and every background M1 supports
          // is fully opaque where it paints at all.
          if let Some(color) = get("background-color").and_then(|v| parse_color_value(v, colors)) {
              style.background = Background::Solid(color);
          }
          if let Some(value) = get("background-image") {
              if value.trim() != "none" {
                  if let Some(background) = parse_background_image(value, colors) {
                      style.background = background;
                  }
              }
          }

          // `border: <width> <style> [<color>]` -- Adwaita writes `1px solid`
          // with no color and sets `border-color` separately.
          if let Some(value) = get("border") {
              for part in value.split_whitespace() {
                  if let Some(width) = parse_px(part) {
                      style.border_width = width;
                  } else if let Some(color) = parse_color_value(part, colors) {
                      style.border_color = color;
                  }
              }
          }
          if let Some(width) = get("border-width").and_then(parse_px) {
              style.border_width = width;
          }
          if let Some(color) = get("border-color").and_then(|v| parse_color_value(v, colors)) {
              style.border_color = color;
          }
          if let Some(radius) = get("border-radius").and_then(parse_px) {
              style.border_radius = radius;
          }
          if let Some(padding) = get("padding").and_then(parse_padding) {
              style.padding = padding;
          }
          if let Some(min_width) = get("min-width").and_then(parse_px) {
              style.min_width = min_width;
          }
          if let Some(min_height) = get("min-height").and_then(parse_px) {
              style.min_height = min_height;
          }
          if let Some(font_size) = get("font-size").and_then(parse_px) {
              style.font_size = font_size;
          }

          style
      }
  }
  ```

- [ ] **Add to `ui/src/css/mod.rs`:** `pub mod computed;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```

- [ ] **Commit:**
  ```bash
  git add ui/src/css
  git commit -m "$(cat <<'EOF'
  feat(ui): compute the button properties the slice paints

  Background is an enum, not a Color: Adwaita's buttons carry no
  background-color at all -- every state is a background-image, either
  GTK's image(<color>) flat fill or a two-stop linear-gradient(to top, ...).
  color_at() samples a `to top` gradient from the bottom edge, so the
  1px/2px stop offsets Adwaita uses land where CSS says they do.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 7 — Text: load a system typeface, shape and measure a label

**Files**
- Create: `ui/src/text.rs`
- Modify: `ui/src/lib.rs` (add `pub mod text;`)
- Test: `ui/src/text.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: nothing from earlier tasks.
- Produces:
  ```rust
  pub const FONT_CANDIDATES: &[&str];
  pub struct FontStack { typeface: Arc<Typeface>, shaper: Shaper }
  impl FontStack {
      pub fn system() -> Option<FontStack>;
      pub fn from_file(path: &Path) -> Option<FontStack>;
      pub fn font(&self, size_px: f32) -> Font;
      pub fn measure(&self, text: &str, size_px: f32) -> TextMetrics;
      pub fn blob(&self, text: &str, size_px: f32) -> Option<TextBlob>;
  }
  pub struct TextMetrics { pub width: f32, pub ascent: f32, pub descent: f32, pub line_height: f32 }
  ```
  `blob` returns glyph runs positioned relative to `(0, 0)` on the baseline; the painter translates.

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/text.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::FontStack;

      #[test]
      fn a_system_typeface_is_found_without_fontconfig() {
          let stack = FontStack::system()
              .expect("no font found among FONT_CANDIDATES; install dejavu/liberation/noto sans");
          assert!(!stack.family_name().is_empty());
      }

      #[test]
      fn measurement_scales_with_size_and_length() {
          let stack = FontStack::system().expect("system font");
          let small = stack.measure("Click me", 14.0);
          let large = stack.measure("Click me", 28.0);
          assert!(small.width > 0.0, "zero-width measurement means no hmtx data reached us");
          assert!(
              large.width > small.width * 1.8,
              "28px measured {} vs 14px {}: advances are not scaling with size",
              large.width,
              small.width
          );
          let longer = stack.measure("Click me twice", 14.0);
          assert!(longer.width > small.width);

          assert!(small.ascent > 0.0 && small.descent > 0.0);
          assert!(small.line_height >= small.ascent + small.descent);
      }

      #[test]
      fn shaping_produces_one_positioned_glyph_per_character() {
          let stack = FontStack::system().expect("system font");
          let blob = stack.blob("Click me", 14.0).expect("shaping produced no runs");
          let glyphs: usize = blob.runs().iter().map(|r| r.glyphs.len()).sum();
          assert_eq!(
              glyphs,
              "Click me".chars().count(),
              "Latin text with no ligatures should shape 1:1"
          );
          for run in blob.runs() {
              assert_eq!(run.glyphs.len(), run.positions.len());
          }
          // Positions must advance left to right.
          let run = &blob.runs()[0];
          for pair in run.positions.windows(2) {
              assert!(pair[1].x > pair[0].x, "glyph positions did not advance");
          }
      }

      #[test]
      fn blob_width_agrees_with_measure() {
          let stack = FontStack::system().expect("system font");
          let metrics = stack.measure("Click me", 14.0);
          let blob = stack.blob("Click me", 14.0).expect("blob");
          let last = blob.runs()[0].positions.last().copied().expect("positions");
          assert!(
              last.x < metrics.width && last.x > metrics.width * 0.5,
              "last glyph origin {} is not inside the measured width {}",
              last.x,
              metrics.width
          );
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui text::
  ```
  Expected: `error[E0583]: file not found for module 'text'`.

- [ ] **Write `ui/src/text.rs`:**
  ```rust
  //! The text stack: `skia-rs-text`, no fontconfig, no `cosmic-text`.
  //!
  //! M1's resolution of the spec's "text stack decision" risk. `skia-rs-text`
  //! already loads a TTF/OTF from raw bytes (`ttf_parser`), shapes with
  //! `rustybuzz`, reports real `hmtx` advances, and rasterizes glyph outlines
  //! through `Canvas::draw_text_blob` -- so the whole shape/measure/draw path
  //! is one crate and `cosmic-text` buys nothing here.
  //!
  //! Font *discovery* is deliberately a fixed probe list rather than
  //! fontconfig: M1 needs one predictable UI face, and linking fontconfig
  //! would re-import exactly the platform dependency this rebuild is leaving.
  //! Real font matching (family/weight/style from the theme's `font-family`)
  //! is M2/M3 work.

  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  use skia_rs_safe::core::Point;
  use skia_rs_safe::text::{Font, Shaper, TextBlob, TextBlobBuilder, Typeface};

  /// Well-known UI sans-serif faces, in preference order.
  pub const FONT_CANDIDATES: &[&str] = &[
      "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
      "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
      "/usr/share/fonts/noto/NotoSans-Regular.ttf",
      "/usr/share/fonts/TTF/DejaVuSans.ttf",
      "/usr/share/fonts/dejavu/DejaVuSans.ttf",
      "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
      "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
      "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
  ];

  /// Measured extents of a laid-out string.
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct TextMetrics {
      /// Total advance width in px.
      pub width: f32,
      /// Distance from the baseline to the top of the text, positive upward.
      pub ascent: f32,
      /// Distance from the baseline to the bottom of the text, positive downward.
      pub descent: f32,
      /// Recommended distance between successive baselines.
      pub line_height: f32,
  }

  /// A loaded typeface plus a shaper.
  pub struct FontStack {
      typeface: Arc<Typeface>,
      shaper: Shaper,
  }

  impl FontStack {
      /// Load the first readable, parseable face from [`FONT_CANDIDATES`].
      #[must_use]
      pub fn system() -> Option<Self> {
          for candidate in FONT_CANDIDATES {
              let path = PathBuf::from(candidate);
              if let Some(stack) = Self::from_file(&path) {
                  tracing::debug!(font = %path.display(), "loaded UI typeface");
                  return Some(stack);
              }
          }
          tracing::warn!(candidates = FONT_CANDIDATES.len(), "no UI typeface found");
          None
      }

      /// Load a specific font file.
      #[must_use]
      pub fn from_file(path: &Path) -> Option<Self> {
          let data = std::fs::read(path).ok()?;
          let typeface = Typeface::from_data(data)?;
          Some(Self {
              typeface: Arc::new(typeface),
              shaper: Shaper::new(),
          })
      }

      /// The loaded face's family name.
      #[must_use]
      pub fn family_name(&self) -> &str {
          self.typeface.family_name()
      }

      /// A [`Font`] at `size_px`.
      #[must_use]
      pub fn font(&self, size_px: f32) -> Font {
          Font::new(Arc::clone(&self.typeface), size_px)
      }

      /// Measure `text` at `size_px`.
      ///
      /// Width comes from the shaper's advances when shaping succeeds (so
      /// kerning and ligatures count), and from `Font::measure_text`'s
      /// per-glyph `hmtx` sum otherwise.
      #[must_use]
      pub fn measure(&self, text: &str, size_px: f32) -> TextMetrics {
          let font = self.font(size_px);
          let metrics = font.metrics();
          let width = match self.shaper.shape_auto(text, &font) {
              Some(runs) if !runs.is_empty() => runs.iter().map(|run| run.width).sum(),
              _ => font.measure_text(text),
          };
          TextMetrics {
              width,
              // `FontMetrics::ascent` is negative (above the baseline), matching Skia.
              ascent: -metrics.ascent,
              descent: metrics.descent,
              line_height: metrics.line_height(),
          }
      }

      /// Shape `text` into a blob whose glyph positions are relative to the
      /// origin `(0, 0)` on the baseline.
      #[must_use]
      pub fn blob(&self, text: &str, size_px: f32) -> Option<TextBlob> {
          let font = self.font(size_px);
          let runs = self.shaper.shape_auto(text, &font)?;
          let mut builder = TextBlobBuilder::new();
          let mut pen_x = 0.0f32;
          let mut any = false;
          for run in &runs {
              let mut glyphs = Vec::with_capacity(run.glyphs.len());
              let mut positions = Vec::with_capacity(run.glyphs.len());
              for glyph in &run.glyphs {
                  glyphs.push(glyph.glyph_id.0);
                  positions.push(Point::new(pen_x + glyph.x_offset, glyph.y_offset));
                  pen_x += glyph.x_advance;
              }
              if !glyphs.is_empty() {
                  builder.add_positioned_run(&run.font, &glyphs, &positions);
                  any = true;
              }
          }
          if !any {
              return None;
          }
          builder.build()
      }
  }
  ```
- [ ] **Add to `ui/src/lib.rs`:** `pub mod text;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```

- [ ] **Commit:**
  ```bash
  git add ui/src/lib.rs ui/src/text.rs
  git commit -m "$(cat <<'EOF'
  feat(ui): shape and measure labels with skia-rs-text

  Resolves the spec's text-stack risk in favour of skia-rs-text over
  cosmic-text: Typeface::from_data loads a TTF straight from disk,
  Shaper::shape_auto shapes via rustybuzz with real hmtx advances, and
  TextBlobBuilder feeds Canvas::draw_text_blob -- one crate for the whole
  load/shape/measure/draw path. Font discovery is a fixed probe list, not
  fontconfig, which is the platform dependency this rebuild is leaving.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 8 — Layout: `taffy` turns style + label size into an allocation

**Files**
- Create: `ui/src/layout.rs`
- Modify: `ui/src/lib.rs` (add `pub mod layout;`)
- Test: `ui/src/layout.rs` (in-crate `#[cfg(test)] mod tests`)

**Interfaces**
- Consumes: `css::computed::ComputedStyle`, `text::TextMetrics`.
- Produces:
  ```rust
  pub struct Allocation { pub width: f32, pub height: f32, pub label_x: f32, pub label_y: f32 }
  pub fn layout_button(style: &ComputedStyle, label: &TextMetrics) -> Allocation;
  ```
  `label_x`/`label_y` are the label content box's top-left corner inside the button's border box; the painter adds the baseline offset.

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/layout.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::layout_button;
      use crate::css::computed::{Background, ComputedStyle};
      use crate::text::TextMetrics;
      use skia_rs_safe::core::Color;

      fn adwaita_like() -> ComputedStyle {
          ComputedStyle {
              background: Background::Solid(Color(0xFFDA_D6D2)),
              color: Color(0xFF2E_3436),
              border_width: 1.0,
              border_color: Color(0xFFCD_C7C2),
              border_radius: 5.0,
              padding: [4.0, 9.0, 4.0, 9.0],
              min_width: 16.0,
              min_height: 24.0,
              font_size: 14.0,
          }
      }

      fn label(width: f32, height: f32) -> TextMetrics {
          TextMetrics {
              width,
              ascent: height * 0.8,
              descent: height * 0.2,
              line_height: height,
          }
      }

      #[test]
      fn allocation_is_text_plus_padding_plus_border() {
          let allocation = layout_button(&adwaita_like(), &label(60.0, 18.0));
          // 60 + 9 + 9 + 1 + 1
          assert_eq!(allocation.width, 80.0);
          // 18 + 4 + 4 + 1 + 1
          assert_eq!(allocation.height, 28.0);
          assert_eq!(allocation.label_x, 10.0, "border 1 + padding-left 9");
          assert_eq!(allocation.label_y, 5.0, "border 1 + padding-top 4");
      }

      #[test]
      fn min_size_floors_a_tiny_label() {
          // "" is 0 wide and 6 tall: min-width 16 / min-height 24 must win.
          let allocation = layout_button(&adwaita_like(), &label(0.0, 6.0));
          assert_eq!(allocation.width, 20.0, "max(0 + 18 + 2, min-width 16) == 20");
          assert_eq!(allocation.height, 24.0, "max(6 + 8 + 2, min-height 24) == 24");
      }

      #[test]
      fn a_borderless_paddingless_button_is_exactly_the_label() {
          let style = ComputedStyle {
              padding: [0.0; 4],
              border_width: 0.0,
              min_width: 0.0,
              min_height: 0.0,
              ..adwaita_like()
          };
          let allocation = layout_button(&style, &label(42.0, 17.0));
          assert_eq!(allocation.width, 42.0);
          assert_eq!(allocation.height, 17.0);
          assert_eq!(allocation.label_x, 0.0);
          assert_eq!(allocation.label_y, 0.0);
      }

      #[test]
      fn the_label_is_centred_when_min_size_grows_the_button() {
          let allocation = layout_button(&adwaita_like(), &label(0.0, 6.0));
          // Content box is 24 - 2 - 8 = 14 tall; a 6-tall label centres at 4
          // inside it, i.e. 1 (border) + 4 (padding) + 4 == 9 from the top.
          assert_eq!(allocation.label_y, 9.0);
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui layout::
  ```
  Expected: `error[E0583]: file not found for module 'layout'`.

- [ ] **Write `ui/src/layout.rs`.** The label is a real taffy leaf child of a flex container, so taffy — not hand arithmetic — does the box-model sum and the min-size clamp; `Style::size` is a *border-box* size in taffy 0.14 (`box_sizing` defaults to `BorderBox`), which is why the intrinsic size goes on the child rather than on the button:
  ```rust
  //! GTK's measure→allocate box model, expressed as a `taffy` tree.
  //!
  //! The button is a flex container carrying the computed padding, border and
  //! min-size; the label is a leaf child sized to its shaped extents. Taffy
  //! then produces the border-box allocation and the label's position inside
  //! it, including the min-size clamp and the centring -- none of which this
  //! crate reimplements.

  use taffy::prelude::{
      length, AlignItems, AvailableSpace, Display, JustifyContent, Size, Style, TaffyTree,
  };

  use crate::css::computed::ComputedStyle;
  use crate::text::TextMetrics;

  /// A laid-out button: its border-box size and its label's position within it.
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct Allocation {
      /// Border-box width in px.
      pub width: f32,
      /// Border-box height in px.
      pub height: f32,
      /// Label content box's left edge, relative to the button's left edge.
      pub label_x: f32,
      /// Label content box's top edge, relative to the button's top edge.
      pub label_y: f32,
  }

  /// Lay out a button whose label measures `label`.
  ///
  /// Panics only if `taffy` itself fails, which for a two-node tree built
  /// entirely from finite lengths means a bug in this function, not bad input.
  #[must_use]
  pub fn layout_button(style: &ComputedStyle, label: &TextMetrics) -> Allocation {
      let [pad_top, pad_right, pad_bottom, pad_left] = style.padding;
      let border = style.border_width;

      let mut tree: TaffyTree<()> = TaffyTree::new();
      let label_node = tree
          .new_leaf(Style {
              size: Size {
                  width: length(label.width),
                  height: length(label.line_height),
              },
              ..Style::default()
          })
          .expect("taffy leaf");
      let button_node = tree
          .new_with_children(
              Style {
                  display: Display::Flex,
                  align_items: Some(AlignItems::CENTER),
                  justify_content: Some(JustifyContent::CENTER),
                  min_size: Size {
                      width: length(style.min_width),
                      height: length(style.min_height),
                  },
                  padding: taffy::geometry::Rect {
                      left: length(pad_left),
                      right: length(pad_right),
                      top: length(pad_top),
                      bottom: length(pad_bottom),
                  },
                  border: taffy::geometry::Rect {
                      left: length(border),
                      right: length(border),
                      top: length(border),
                      bottom: length(border),
                  },
                  ..Style::default()
              },
              &[label_node],
          )
          .expect("taffy container");

      tree.compute_layout(
          button_node,
          Size {
              width: AvailableSpace::MaxContent,
              height: AvailableSpace::MaxContent,
          },
      )
      .expect("taffy layout");

      let button = *tree.layout(button_node).expect("button layout");
      let label_layout = *tree.layout(label_node).expect("label layout");

      Allocation {
          width: button.size.width,
          height: button.size.height,
          label_x: label_layout.location.x,
          label_y: label_layout.location.y,
      }
  }
  ```
  Note `AlignItems::CENTER` / `JustifyContent::CENTER`: in taffy 0.14 these are **structs** (position keyword + overflow-safety modifier) with associated constants, not the plain enums earlier versions exposed.

- [ ] **Add to `ui/src/lib.rs`:** `pub mod layout;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```

- [ ] **Commit:**
  ```bash
  git add ui/src/lib.rs ui/src/layout.rs
  git commit -m "$(cat <<'EOF'
  feat(ui): allocate the button with taffy

  The label is a real taffy leaf child of a flex container carrying the
  computed padding/border/min-size, so taffy does the box-model sum, the
  min-size clamp and the centring rather than this crate reimplementing
  them. Intrinsic size goes on the child because taffy 0.14's Style::size
  is a border-box size (box_sizing defaults to BorderBox).

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 9 — Paint, the `Button` widget, and **the load-bearing test**

**Files**
- Create: `ui/src/paint.rs`, `ui/src/widget/mod.rs`, `ui/src/widget/button.rs`, `ui/tests/themed_button_offscreen.rs`
- Modify: `ui/src/lib.rs` (add `pub mod paint; pub mod widget;`)
- Test: `ui/tests/themed_button_offscreen.rs`

**Interfaces**
- Consumes: `css::cascade::CompiledSheet`, `css::computed::{Background, ComputedStyle}`, `css::select::{CssNode, PseudoStates}`, `layout::{Allocation, layout_button}`, `text::FontStack`.
- Produces:
  ```rust
  // paint.rs
  pub fn paint_button(surface: &mut Surface, origin: (f32, f32), style: &ComputedStyle,
                      allocation: &Allocation, label: &str, fonts: &FontStack);
  // widget/button.rs
  pub struct Button { /* label, classes, node, states, style, allocation */ }
  impl Button {
      pub fn new(label: &str, classes: &[&str], parent: CssNode) -> Button;
      pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &FontStack);
      pub fn set_states(&mut self, states: PseudoStates, sheet: &CompiledSheet, fonts: &FontStack);
      pub fn states(&self) -> PseudoStates;
      pub fn style(&self) -> &ComputedStyle;
      pub fn allocation(&self) -> Allocation;
      pub fn render(&self, surface: &mut Surface, origin: (f32, f32), fonts: &FontStack);
      pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool;
  }
  ```

### Steps

- [ ] **Write the failing load-bearing test** `ui/tests/themed_button_offscreen.rs`:
  ```rust
  //! The M1 load-bearing gate: CSS -> cascade -> computed values -> Skia
  //! pixels, with no compositor involved.
  //!
  //! Every expected value below is read out of the vendored Adwaita sheet,
  //! so this test fails if any seam in the pipeline is wrong -- a mis-parsed
  //! declaration, a selector that stops matching, a cascade that picks the
  //! wrong winner, a gradient sampled from the wrong edge, or a paint that
  //! puts the wrong bytes down.

  use icedtea_ui::css::cascade::CompiledSheet;
  use icedtea_ui::css::computed::{Background, ComputedStyle, GradientStop};
  use icedtea_ui::css::select::{CssNode, PseudoStates};
  use icedtea_ui::text::FontStack;
  use icedtea_ui::widget::button::Button;
  use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
  use skia_rs_safe::canvas::Surface;
  use skia_rs_safe::core::Color;

  const SURFACE_W: i32 = 240;
  const SURFACE_H: i32 = 80;

  fn stop(color: u32, position_px: Option<f32>) -> GradientStop {
      GradientStop {
          color: Color(color),
          position_px,
      }
  }

  /// A window > button node tree, a compiled Adwaita sheet and a system font.
  fn fixture(classes: &[&str]) -> (CompiledSheet, FontStack, Button) {
      let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
      let fonts = FontStack::system()
          .expect("no system font found; install dejavu/liberation/noto sans");
      let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
      let mut button = Button::new("Click me", classes, window);
      button.restyle(&sheet, &fonts);
      (sheet, fonts, button)
  }

  /// Render `button` at the surface origin and return the surface.
  fn render(button: &Button, fonts: &FontStack) -> Surface {
      let mut surface =
          Surface::new_raster_n32_premul(SURFACE_W, SURFACE_H).expect("raster surface");
      surface.canvas().clear(Color::TRANSPARENT);
      button.render(&mut surface, (0.0, 0.0), fonts);
      surface
  }

  /// Read a pixel back as premultiplied ARGB.
  fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
      surface
          .pixel_buffer()
          .get_pixel(x, y)
          .unwrap_or_else(|| panic!("pixel ({x}, {y}) is outside the surface"))
  }

  #[test]
  fn adwaita_button_computed_style_and_pixels_match_the_theme() {
      let (sheet, fonts, mut button) = fixture(&[]);

      // --- 1. Computed values equal Adwaita's resolved values ------------
      // `button` (line 215 of the vendored sheet).
      let normal = *button.style();
      assert_eq!(
          normal.background,
          Background::LinearGradientToTop {
              from: stop(0xFFF6_F5F4, Some(2.0)),
              to: stop(0xFFFB_FAFA, None),
          },
          "Adwaita's base button background did not resolve"
      );
      assert_eq!(normal.color, Color(0xFF2E_3436));
      assert_eq!(normal.border_color, Color(0xFFCD_C7C2));
      assert_eq!(normal.border_width, 1.0);
      assert_eq!(normal.border_radius, 5.0);
      assert_eq!(normal.padding, [4.0, 9.0, 4.0, 9.0]);
      assert_eq!(normal.min_width, 16.0);
      assert_eq!(normal.min_height, 24.0);
      assert_eq!(normal.font_size, ComputedStyle::DEFAULT_FONT_SIZE);

      let allocation = button.allocation();
      assert!(
          allocation.width > 40.0 && allocation.width < SURFACE_W as f32,
          "implausible allocation width {}",
          allocation.width
      );
      assert!(allocation.height >= 24.0, "min-height was not honored");

      let cx = (allocation.width / 2.0) as i32;
      let cy = (allocation.height / 2.0) as i32;

      // --- 2. Pixels: the normal state -----------------------------------
      let surface = render(&button, &fonts);

      // `linear-gradient(to top, #f6f5f4 2px, #fbfafa)`: below the 2px first
      // stop the fill is flat, so the row just inside the bottom border is
      // exactly the first stop's color.
      let inner_bottom = (allocation.height - 2.0) as i32;
      assert_eq!(
          pixel(&surface, cx, inner_bottom),
          Color(0xFFF6_F5F4),
          "the flat pre-first-stop band of Adwaita's button gradient is wrong"
      );
      let normal_center = pixel(&surface, cx, cy);
      assert!(
          normal_center != Color(0xFFF6_F5F4) && normal_center != Color(0xFFFB_FAFA),
          "the gradient's midpoint {normal_center:?} equals a stop: it is not interpolating"
      );
      assert_eq!(normal_center.alpha(), 255);
      assert!((0xF6..=0xFB).contains(&normal_center.red()));
      assert!((0xF5..=0xFA).contains(&normal_center.green()));
      assert!((0xF4..=0xFA).contains(&normal_center.blue()));

      // A pixel just outside the 5px corner radius is transparent: (0, 0) is
      // ~7.07px from the corner arc's centre, comfortably past r=5 even with
      // anti-aliasing.
      assert_eq!(
          pixel(&surface, 0, 0).alpha(),
          0,
          "the corner outside border-radius: 5px was painted"
      );
      // The border itself is on screen, on the straight run of the top edge.
      assert_eq!(
          pixel(&surface, cx, 0),
          Color(0xFFCD_C7C2),
          "Adwaita's 1px #cdc7c2 button border is missing"
      );

      // --- 3. Toggling :hover changes computed style AND pixels ----------
      button.set_states(
          PseudoStates {
              hover: true,
              ..PseudoStates::default()
          },
          &sheet,
          &fonts,
      );
      let hovered = *button.style();
      assert_eq!(
          hovered.background,
          Background::LinearGradientToTop {
              from: stop(0xFFD6_D1CD, None),
              to: stop(0xFFE8_E6E3, Some(1.0)),
          },
          "`button:hover` did not win the cascade"
      );
      assert_ne!(hovered.background, normal.background);

      let hovered_surface = render(&button, &fonts);
      // The second stop sits 1px above the bottom edge, so everything above
      // it -- the whole centre -- is flat #e8e6e3.
      assert_eq!(
          pixel(&hovered_surface, cx, cy),
          Color(0xFFE8_E6E3),
          "the hover gradient's post-last-stop band is wrong"
      );
      assert_ne!(
          pixel(&hovered_surface, cx, cy),
          normal_center,
          "toggling :hover did not change the painted pixels"
      );

      // --- 4. Toggling :active: a flat `image(<color>)` background --------
      button.set_states(
          PseudoStates {
              active: true,
              ..PseudoStates::default()
          },
          &sheet,
          &fonts,
      );
      assert_eq!(
          button.style().background,
          Background::Solid(Color(0xFFDA_D6D2)),
          "`button:active`'s image(#dad6d2) did not resolve to a flat fill"
      );
      let active_surface = render(&button, &fonts);
      assert_eq!(
          pixel(&active_surface, cx, cy),
          Color(0xFFDA_D6D2),
          "the :active flat background is not the color the theme declares"
      );
      assert_eq!(pixel(&active_surface, 0, 0).alpha(), 0);
  }

  #[test]
  fn suggested_action_button_is_adwaitas_accent_blue() {
      let (_sheet, fonts, button) = fixture(&["suggested-action"]);
      let style = *button.style();
      assert_eq!(
          style.background,
          Background::LinearGradientToTop {
              from: stop(0xFF2C_7FE3, Some(2.0)),
              to: stop(0xFF35_84E4, None),
          }
      );
      assert_eq!(style.color, Color(0xFFFF_FFFF), "suggested-action text is white");
      assert_eq!(style.border_color, Color(0xFF15_539E));

      let allocation = button.allocation();
      let surface = render(&button, &fonts);
      let center = pixel(
          &surface,
          (allocation.width / 2.0) as i32,
          (allocation.height / 2.0) as i32,
      );
      // Both stops are within 9/5/1 per channel of `@accent_color` #3584E4,
      // so any point on the gradient is close to it.
      let close = |a: u8, b: u8| i32::from(a).abs_diff(i32::from(b)) <= 10;
      assert!(
          close(center.red(), 0x35) && close(center.green(), 0x84) && close(center.blue(), 0xE4),
          "suggested-action centre {center:?} is not Adwaita's accent blue"
      );
  }

  #[test]
  fn the_label_is_actually_drawn() {
      let (_sheet, fonts, button) = fixture(&[]);
      let allocation = button.allocation();
      let surface = render(&button, &fonts);

      // The label is #2e3436 on a near-white background; count pixels
      // markedly darker than the lightest gradient stop inside the content
      // box. Zero would mean the glyph run never reached the canvas.
      let mut dark = 0u32;
      for y in 6..(allocation.height as i32 - 6) {
          for x in 10..(allocation.width as i32 - 10) {
              if pixel(&surface, x, y).red() < 0xC0 {
                  dark += 1;
              }
          }
      }
      assert!(dark > 20, "only {dark} label pixels were painted");
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui --test themed_button_offscreen
  ```
  Expected: `error[E0432]: unresolved import 'icedtea_ui::widget'`.

- [ ] **Write `ui/src/paint.rs`.** Skia's raster canvas has `draw_round_rect(&Rect, rx, ry, &Paint)` — there is no `draw_rrect`; the gradient is painted as horizontal bands clipped to the rounded rect, which keeps the painted color exactly `Background::color_at`'s value for every row and so keeps the pixel assertions derivable from the CSS alone:
  ```rust
  //! Skia paint for one themed button.
  //!
  //! Backgrounds are painted row-band by row-band under a rounded-rect clip
  //! rather than through a gradient shader: `Background::color_at` is then the
  //! single definition of what color lands on any given row, so a pixel
  //! assertion is derivable straight from the theme's declaration.

  use skia_rs_safe::canvas::{ClipOp, Surface};
  use skia_rs_safe::core::Rect;
  use skia_rs_safe::paint::{Paint, Style};
  use skia_rs_safe::path::PathBuilder;

  use crate::css::computed::{Background, ComputedStyle};
  use crate::layout::Allocation;
  use crate::text::FontStack;

  /// Paint `label` in a button of `allocation` at `origin`, styled by `style`.
  pub fn paint_button(
      surface: &mut Surface,
      origin: (f32, f32),
      style: &ComputedStyle,
      allocation: &Allocation,
      label: &str,
      fonts: &FontStack,
  ) {
      let (ox, oy) = origin;
      let radius = style.border_radius;
      let border = style.border_width;

      // --- background: the padding box, i.e. inside the border ------------
      let bg = Rect::from_xywh(
          ox + border,
          oy + border,
          (allocation.width - border * 2.0).max(0.0),
          (allocation.height - border * 2.0).max(0.0),
      );
      let bg_radius = (radius - border).max(0.0);
      let bg_height = bg.bottom - bg.top;

      match style.background {
          Background::Transparent => {}
          Background::Solid(color) => {
              let mut paint = Paint::new();
              paint.set_color32(color);
              paint.set_style(Style::Fill);
              paint.set_anti_alias(true);
              surface
                  .canvas()
                  .draw_round_rect(&bg, bg_radius, bg_radius, &paint);
          }
          Background::LinearGradientToTop { .. } => {
              let mut canvas = surface.canvas();
              let save = canvas.save();
              // Clip to the rounded padding box, then fill 1px bands.
              let mut clip = PathBuilder::new();
              clip.add_round_rect(&bg, bg_radius, bg_radius);
              canvas.clip_path(&clip.build(), ClipOp::Intersect, true);
              let rows = bg_height.ceil() as i32;
              for row in 0..rows {
                  let y = bg.top + row as f32;
                  let color = style
                      .background
                      .color_at(bg_height, y - bg.top + 0.5);
                  let mut paint = Paint::new();
                  paint.set_color32(color);
                  paint.set_style(Style::Fill);
                  paint.set_anti_alias(false);
                  canvas.draw_rect(
                      &Rect::new(bg.left, y, bg.right, (y + 1.0).min(bg.bottom)),
                      &paint,
                  );
              }
              canvas.restore_to_count(save);
          }
      }

      // --- border: stroked on the centre line of the border ---------------
      if border > 0.0 && style.border_color.alpha() > 0 {
          let half = border / 2.0;
          let outline = Rect::from_xywh(
              ox + half,
              oy + half,
              (allocation.width - border).max(0.0),
              (allocation.height - border).max(0.0),
          );
          let outline_radius = (radius - half).max(0.0);
          let mut paint = Paint::new();
          paint.set_color32(style.border_color);
          paint.set_style(Style::Stroke);
          paint.set_stroke_width(border);
          paint.set_anti_alias(true);
          surface
              .canvas()
              .draw_round_rect(&outline, outline_radius, outline_radius, &paint);
      }

      // --- label ----------------------------------------------------------
      if !label.is_empty() {
          if let Some(blob) = fonts.blob(label, style.font_size) {
              let metrics = fonts.measure(label, style.font_size);
              let mut paint = Paint::new();
              paint.set_color32(style.color);
              paint.set_style(Style::Fill);
              paint.set_anti_alias(true);
              surface.canvas().draw_text_blob(
                  &blob,
                  ox + allocation.label_x,
                  oy + allocation.label_y + metrics.ascent,
                  &paint,
              );
          }
      }
  }

  ```
  `PathBuilder::add_round_rect(&Rect, rx, ry)` and `Canvas::clip_path(&Path, ClipOp, do_anti_alias)` are both already in `skia-rs-path` / `skia-rs-canvas`, so the clip needs no hand-rolled arc geometry.

- [ ] **Write `ui/src/widget/mod.rs`:**
  ```rust
  //! The bespoke widget layer: widgets that wear GTK's CSS node identity.

  pub mod button;
  ```

- [ ] **Write `ui/src/widget/button.rs`:**
  ```rust
  //! The M1 widget: a `button` CSS node with interaction state.

  use skia_rs_safe::canvas::Surface;

  use crate::css::cascade::CompiledSheet;
  use crate::css::computed::ComputedStyle;
  use crate::css::select::{CssNode, PseudoStates};
  use crate::layout::{layout_button, Allocation};
  use crate::paint::paint_button;
  use crate::text::FontStack;

  /// A themed button: CSS node identity, interaction state, computed style
  /// and allocation, kept together and recomputed on every state change.
  pub struct Button {
      label: String,
      node: CssNode,
      style: ComputedStyle,
      allocation: Allocation,
  }

  impl Button {
      /// Create a `button` node with `classes`, parented to `parent`.
      ///
      /// The style and allocation are the type's defaults until
      /// [`restyle`](Self::restyle) runs.
      #[must_use]
      pub fn new(label: &str, classes: &[&str], parent: CssNode) -> Self {
          Self {
              label: label.to_string(),
              node: CssNode::new("button", classes, PseudoStates::default(), Some(parent)),
              style: ComputedStyle::default(),
              allocation: Allocation {
                  width: 0.0,
                  height: 0.0,
                  label_x: 0.0,
                  label_y: 0.0,
              },
          }
      }

      /// Re-run cascade, measurement and layout for the current state.
      pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &FontStack) {
          self.style = ComputedStyle::resolve(sheet, &self.node);
          let metrics = fonts.measure(&self.label, self.style.font_size);
          self.allocation = layout_button(&self.style, &metrics);
      }

      /// Replace the pseudo-class state and restyle.
      pub fn set_states(
          &mut self,
          states: PseudoStates,
          sheet: &CompiledSheet,
          fonts: &FontStack,
      ) {
          self.node = self.node.with_states(states);
          self.restyle(sheet, fonts);
      }

      /// The current pseudo-class state.
      #[must_use]
      pub fn states(&self) -> PseudoStates {
          self.node.states()
      }

      /// The computed style from the last [`restyle`](Self::restyle).
      #[must_use]
      pub fn style(&self) -> &ComputedStyle {
          &self.style
      }

      /// The allocation from the last [`restyle`](Self::restyle).
      #[must_use]
      pub fn allocation(&self) -> Allocation {
          self.allocation
      }

      /// Paint this button at `origin` on `surface`.
      pub fn render(&self, surface: &mut Surface, origin: (f32, f32), fonts: &FontStack) {
          paint_button(
              surface,
              origin,
              &self.style,
              &self.allocation,
              &self.label,
              fonts,
          );
      }

      /// Whether surface point `(x, y)` falls inside this button's border box
      /// when the button is drawn at `origin`. Rectangular, not radius-aware:
      /// GTK's own hit testing is rectangular too.
      #[must_use]
      pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool {
          let (ox, oy) = (f64::from(origin.0), f64::from(origin.1));
          x >= ox
              && y >= oy
              && x < ox + f64::from(self.allocation.width)
              && y < oy + f64::from(self.allocation.height)
      }
  }
  ```

- [ ] **Add to `ui/src/lib.rs`:** `pub mod paint;` and `pub mod widget;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
  Expected: `themed_button_offscreen` reports `3 passed`, including `adwaita_button_computed_style_and_pixels_match_the_theme`. **This is the M1 gate.** If a pixel assertion is off by a single unit, do not relax it — find the seam: check that `Background::color_at` is being sampled with `y - bg.top + 0.5` (row centre, in padding-box coordinates), that `bg_height` is the padding box's height and not the border box's, and that the border stroke is not overlapping the row being sampled.

- [ ] **Commit:**
  ```bash
  git add ui/src/lib.rs ui/src/paint.rs ui/src/widget ui/tests/themed_button_offscreen.rs
  git commit -m "$(cat <<'EOF'
  feat(ui): paint the themed button and gate M1 on its pixels

  The load-bearing test: computed background/color/radius/border/padding
  equal Adwaita's resolved values, the centre pixel equals the color the
  theme declares for that state, a pixel outside border-radius: 5px is
  transparent, and toggling :hover / :active changes both the computed
  style and the painted pixels.

  Gradients are painted as 1px bands under a rounded-rect clip rather than
  via a shader, so Background::color_at is the single definition of what
  lands on a row and every pixel assertion stays derivable from the CSS.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 10 — Wayland: `wl_shm` buffer and a `zwlr_layer_shell_v1` overlay

**Files**
- Create: `ui/src/shm.rs`, `ui/src/wayland.rs`
- Modify: `ui/src/lib.rs` (add `pub mod shm; pub mod wayland;`)
- Test: `ui/src/shm.rs` (in-crate `#[cfg(test)] mod tests` — the buffer *conversion* is unit-testable without a compositor; the protocol path is covered by Task 11's harness test)

**Interfaces**
- Consumes: `widget::button::Button`, `css::select::PseudoStates`, `css::cascade::CompiledSheet`, `text::FontStack`.
- Produces:
  ```rust
  // shm.rs
  pub fn skia_rgba_to_shm_argb(src: &[u8], dst: &mut [u8]);       // R/B swap, premultiplied both sides
  pub struct ShmBuffer { file: File, pool: WlShmPool, buffer: WlBuffer, width: i32, height: i32 }
  impl ShmBuffer {
      pub fn new(shm: &WlShm, qh: &QueueHandle<AppState>, width: i32, height: i32) -> ShmBuffer;
      pub fn wl_buffer(&self) -> &WlBuffer;
      pub fn upload(&mut self, surface: &Surface) -> std::io::Result<()>;
  }
  // wayland.rs
  pub struct AppState { /* globals, pointer state, button, sheet, fonts, dirty flags */ }
  pub struct LayerWindow { conn: Connection, queue: EventQueue<AppState>, state: AppState, ... }
  impl LayerWindow {
      pub fn open(sheet: CompiledSheet, fonts: FontStack, button: Button) -> Result<LayerWindow, LayerWindowError>;
      pub fn run(&mut self) -> Result<(), LayerWindowError>;
  }
  pub enum LayerWindowError { Connect(ConnectError), MissingGlobal(&'static str), Io(std::io::Error), Dispatch(DispatchError) }
  ```

### Steps

- [ ] **Write the failing test** at the bottom of `ui/src/shm.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::skia_rgba_to_shm_argb;
      use skia_rs_safe::canvas::Surface;
      use skia_rs_safe::core::{Color, Rect};
      use skia_rs_safe::paint::{Paint, Style};

      #[test]
      fn rgba_becomes_argb8888_byte_order() {
          // One opaque pixel: R=0x35 G=0x84 B=0xE4 A=0xFF in Skia's physical
          // RGBA order. wl_shm's Argb8888 is little-endian 0xAARRGGBB, i.e.
          // B, G, R, A in memory.
          let src = [0x35u8, 0x84, 0xE4, 0xFF];
          let mut dst = [0u8; 4];
          skia_rgba_to_shm_argb(&src, &mut dst);
          assert_eq!(dst, [0xE4, 0x84, 0x35, 0xFF]);
      }

      #[test]
      fn premultiplied_alpha_passes_through_untouched() {
          // Skia's buffer is already premultiplied and so is Argb8888, so the
          // conversion must not divide or multiply anything.
          let src = [0x40u8, 0x20, 0x10, 0x80];
          let mut dst = [0u8; 4];
          skia_rgba_to_shm_argb(&src, &mut dst);
          assert_eq!(dst, [0x10, 0x20, 0x40, 0x80]);
      }

      #[test]
      fn a_painted_surface_round_trips_to_the_expected_shm_bytes() {
          let mut surface = Surface::new_raster_n32_premul(4, 4).expect("surface");
          surface.canvas().clear(Color::TRANSPARENT);
          let mut paint = Paint::new();
          paint.set_color32(Color(0xFF35_84E4));
          paint.set_style(Style::Fill);
          surface
              .canvas()
              .draw_rect(&Rect::new(0.0, 0.0, 4.0, 4.0), &paint);

          let mut dst = vec![0u8; 4 * 4 * 4];
          skia_rgba_to_shm_argb(surface.pixels(), &mut dst);
          for pixel in dst.chunks_exact(4) {
              assert_eq!(pixel, [0xE4, 0x84, 0x35, 0xFF]);
          }
      }

      #[test]
      fn a_short_destination_is_filled_as_far_as_it_goes() {
          let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
          let mut dst = [0u8; 4];
          skia_rgba_to_shm_argb(&src, &mut dst);
          assert_eq!(dst, [3, 2, 1, 4]);
      }
  }
  ```

- [ ] **Run it — expect a compile failure:**
  ```bash
  cargo test -p icedtea-ui shm::
  ```
  Expected: `error[E0583]: file not found for module 'shm'`.

- [ ] **Write `ui/src/shm.rs`:**
  ```rust
  //! The Skia -> `wl_shm` seam.
  //!
  //! `skia_rs_canvas::Surface` cannot wrap external pixel memory:
  //! `Surface::new_raster` always allocates its own `PixelBuffer`, which is
  //! always physically RGBA with premultiplied alpha and a `width * 4` stride.
  //! `wl_shm`'s `Argb8888` is little-endian `0xAARRGGBB`, i.e. `B, G, R, A` in
  //! memory, also premultiplied. So the seam is one copy with an R/B swap and
  //! no alpha arithmetic at all.

  use std::fs::File;
  use std::io;
  use std::os::fd::{AsFd, OwnedFd};
  use std::os::unix::fs::FileExt;

  use skia_rs_safe::canvas::Surface;
  use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
  use wayland_client::QueueHandle;

  use crate::wayland::AppState;

  /// Convert Skia's physical RGBA-premultiplied bytes into `Argb8888` bytes.
  ///
  /// Copies `min(src.len(), dst.len()) / 4` whole pixels; a partial trailing
  /// pixel is ignored rather than half-written.
  pub fn skia_rgba_to_shm_argb(src: &[u8], dst: &mut [u8]) {
      for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
          d[0] = s[2]; // B
          d[1] = s[1]; // G
          d[2] = s[0]; // R
          d[3] = s[3]; // A
      }
  }

  /// A memfd-backed `wl_shm` buffer in `Argb8888`.
  pub struct ShmBuffer {
      file: File,
      _pool: wl_shm_pool::WlShmPool,
      buffer: wl_buffer::WlBuffer,
      width: i32,
      height: i32,
      scratch: Vec<u8>,
  }

  impl ShmBuffer {
      /// Allocate a `width` x `height` `Argb8888` buffer.
      pub fn new(
          shm: &wl_shm::WlShm,
          qh: &QueueHandle<AppState>,
          width: i32,
          height: i32,
      ) -> io::Result<Self> {
          let stride = width * 4;
          let len = usize::try_from(stride * height)
              .map_err(|_| io::Error::other("negative shm buffer size"))?;
          let fd: OwnedFd =
              rustix::fs::memfd_create("icedtea-ui-shm", rustix::fs::MemfdFlags::CLOEXEC)?;
          rustix::fs::ftruncate(&fd, len as u64)?;
          let file = File::from(fd);
          let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
          let buffer =
              pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
          Ok(Self {
              file,
              _pool: pool,
              buffer,
              width,
              height,
              scratch: vec![0u8; len],
          })
      }

      /// The `wl_buffer` to attach.
      #[must_use]
      pub fn wl_buffer(&self) -> &wl_buffer::WlBuffer {
          &self.buffer
      }

      /// Buffer dimensions in pixels.
      #[must_use]
      pub fn size(&self) -> (i32, i32) {
          (self.width, self.height)
      }

      /// Copy `surface`'s pixels into this buffer.
      ///
      /// Writes through the file rather than an mmap: the pool is `MAP_SHARED`
      /// on the compositor side, so a `pwrite` at offset 0 is visible there,
      /// and this keeps the whole path in safe Rust.
      pub fn upload(&mut self, surface: &Surface) -> io::Result<()> {
          skia_rgba_to_shm_argb(surface.pixels(), &mut self.scratch);
          self.file.write_all_at(&self.scratch, 0)
      }
  }
  ```

- [ ] **Write `ui/src/wayland.rs`.** Registry binding mirrors `harness/src/lib.rs`'s manual `Dispatch` style rather than pulling in `sctk`; the loop is `flush` + `blocking_dispatch`, the "simple poll loop over the Wayland fd" the spec asks for:
  ```rust
  //! The Wayland client: a `zwlr_layer_shell_v1` overlay surface with a
  //! `wl_shm` buffer, driven by a blocking dispatch loop.
  //!
  //! Hand-rolled on `wayland-client` in the same shape the rest of this repo
  //! already uses (see `harness/src/lib.rs`): no `smithay-client-toolkit`, no
  //! `calloop`.

  use wayland_client::protocol::{
      wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
  };
  use wayland_client::{
      delegate_noop, Connection, ConnectError, Dispatch, DispatchError, EventQueue, Proxy,
      QueueHandle,
  };
  use wayland_protocols_wlr::layer_shell::v1::client::{
      zwlr_layer_shell_v1, zwlr_layer_surface_v1,
  };

  use skia_rs_safe::canvas::Surface;
  use skia_rs_safe::core::Color;

  use crate::css::cascade::CompiledSheet;
  use crate::css::select::PseudoStates;
  use crate::shm::ShmBuffer;
  use crate::text::FontStack;
  use crate::widget::button::Button;

  /// Everything that can go wrong opening or running the window.
  #[derive(Debug)]
  pub enum LayerWindowError {
      /// `wl_display` connection failed.
      Connect(ConnectError),
      /// The compositor never advertised a global the client requires.
      MissingGlobal(&'static str),
      /// shm buffer allocation or upload failed.
      Io(std::io::Error),
      /// The event queue failed.
      Dispatch(DispatchError),
  }

  impl std::fmt::Display for LayerWindowError {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          match self {
              Self::Connect(e) => write!(f, "cannot connect to the Wayland display: {e}"),
              Self::MissingGlobal(name) => write!(f, "compositor did not advertise {name}"),
              Self::Io(e) => write!(f, "shm buffer error: {e}"),
              Self::Dispatch(e) => write!(f, "Wayland dispatch failed: {e}"),
          }
      }
  }

  impl std::error::Error for LayerWindowError {}

  /// Client state: bound globals, pointer position, and the widget.
  pub struct AppState {
      compositor: Option<wl_compositor::WlCompositor>,
      shm: Option<wl_shm::WlShm>,
      layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
      seat: Option<wl_seat::WlSeat>,
      pointer: Option<wl_pointer::WlPointer>,
      /// `(width, height)` from the most recent layer-surface `configure`.
      configured: Option<(u32, u32)>,
      /// Set when the widget's appearance changed and the surface needs a repaint.
      dirty: bool,
      /// Set when the compositor asked us to go away.
      closed: bool,
      /// Last pointer position in surface coordinates.
      pointer_at: Option<(f64, f64)>,
      sheet: CompiledSheet,
      fonts: FontStack,
      button: Button,
  }

  impl AppState {
      /// Recompute state from the pointer, restyling only on an actual change.
      fn update_states(&mut self, hover: bool, active: bool) {
          let current = self.button.states();
          if current.hover == hover && current.active == active {
              return;
          }
          self.button.set_states(
              PseudoStates {
                  hover,
                  active,
                  ..PseudoStates::default()
              },
              &self.sheet,
              &self.fonts,
          );
          self.dirty = true;
      }
  }

  /// A mapped layer-shell window showing one themed button.
  pub struct LayerWindow {
      conn: Connection,
      queue: EventQueue<AppState>,
      qh: QueueHandle<AppState>,
      state: AppState,
      surface: wl_surface::WlSurface,
      _layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
      shm_buffer: ShmBuffer,
      skia: Surface,
  }

  /// Margin from the anchored corner, in px. Fixed so a screencopy test knows
  /// exactly where on the output the button lands.
  pub const MARGIN: i32 = 0;

  impl LayerWindow {
      /// Connect, bind globals, map an overlay layer surface sized to the
      /// button, and paint it once.
      pub fn open(
          sheet: CompiledSheet,
          fonts: FontStack,
          mut button: Button,
      ) -> Result<Self, LayerWindowError> {
          button.restyle(&sheet, &fonts);
          let allocation = button.allocation();
          let width = allocation.width.ceil().max(1.0) as i32;
          let height = allocation.height.ceil().max(1.0) as i32;

          let conn = Connection::connect_to_env().map_err(LayerWindowError::Connect)?;
          let display = conn.display();
          let mut queue: EventQueue<AppState> = conn.new_event_queue();
          let qh = queue.handle();
          display.get_registry(&qh, ());

          let mut state = AppState {
              compositor: None,
              shm: None,
              layer_shell: None,
              seat: None,
              pointer: None,
              configured: None,
              dirty: true,
              closed: false,
              pointer_at: None,
              sheet,
              fonts,
              button,
          };
          queue
              .roundtrip(&mut state)
              .map_err(LayerWindowError::Dispatch)?;
          // A second roundtrip: `wl_seat.capabilities` arrives after the bind.
          queue
              .roundtrip(&mut state)
              .map_err(LayerWindowError::Dispatch)?;

          let compositor = state
              .compositor
              .clone()
              .ok_or(LayerWindowError::MissingGlobal("wl_compositor"))?;
          let shm = state
              .shm
              .clone()
              .ok_or(LayerWindowError::MissingGlobal("wl_shm"))?;
          let layer_shell = state
              .layer_shell
              .clone()
              .ok_or(LayerWindowError::MissingGlobal("zwlr_layer_shell_v1"))?;

          let surface = compositor.create_surface(&qh, ());
          let layer_surface = layer_shell.get_layer_surface(
              &surface,
              None,
              zwlr_layer_shell_v1::Layer::Overlay,
              "icedtea-ui-themed-button".to_string(),
              &qh,
              (),
          );
          layer_surface.set_anchor(
              zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left,
          );
          layer_surface.set_size(width as u32, height as u32);
          layer_surface.set_margin(MARGIN, MARGIN, MARGIN, MARGIN);
          layer_surface.set_exclusive_zone(0);
          layer_surface.set_keyboard_interactivity(
              zwlr_layer_surface_v1::KeyboardInteractivity::None,
          );
          surface.commit();
          conn.flush().map_err(LayerWindowError::Io)?;

          while state.configured.is_none() {
              queue
                  .blocking_dispatch(&mut state)
                  .map_err(LayerWindowError::Dispatch)?;
          }

          let shm_buffer =
              ShmBuffer::new(&shm, &qh, width, height).map_err(LayerWindowError::Io)?;
          let skia = Surface::new_raster_n32_premul(width, height)
              .ok_or_else(|| LayerWindowError::Io(std::io::Error::other("raster surface")))?;

          let mut window = Self {
              conn,
              queue,
              qh,
              state,
              surface,
              _layer_surface: layer_surface,
              shm_buffer,
              skia,
          };
          window.repaint()?;
          Ok(window)
      }

      /// Paint the widget, upload it, and commit.
      fn repaint(&mut self) -> Result<(), LayerWindowError> {
          let (width, height) = self.shm_buffer.size();
          self.skia.canvas().clear(Color::TRANSPARENT);
          self.state
              .button
              .render(&mut self.skia, (0.0, 0.0), &self.state.fonts);
          self.shm_buffer
              .upload(&self.skia)
              .map_err(LayerWindowError::Io)?;
          self.surface.attach(Some(self.shm_buffer.wl_buffer()), 0, 0);
          self.surface.damage_buffer(0, 0, width, height);
          self.surface.commit();
          self.conn.flush().map_err(LayerWindowError::Io)?;
          self.state.dirty = false;
          Ok(())
      }

      /// Dispatch until the compositor closes the surface, repainting whenever
      /// pointer state changed the widget's appearance.
      pub fn run(&mut self) -> Result<(), LayerWindowError> {
          while !self.state.closed {
              self.queue
                  .blocking_dispatch(&mut self.state)
                  .map_err(LayerWindowError::Dispatch)?;
              if self.state.dirty {
                  self.repaint()?;
              }
          }
          Ok(())
      }

      /// The queue handle, for callers that need to create further objects.
      #[must_use]
      pub fn queue_handle(&self) -> &QueueHandle<AppState> {
          &self.qh
      }
  }

  impl Dispatch<wl_registry::WlRegistry, ()> for AppState {
      fn event(
          state: &mut Self,
          registry: &wl_registry::WlRegistry,
          event: wl_registry::Event,
          _: &(),
          _: &Connection,
          qh: &QueueHandle<Self>,
      ) {
          if let wl_registry::Event::Global {
              name,
              interface,
              version,
          } = event
          {
              match interface.as_str() {
                  "wl_compositor" => {
                      state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                  }
                  "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                  "zwlr_layer_shell_v1" => {
                      state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                  }
                  "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), qh, ())),
                  _ => {}
              }
          }
      }
  }

  impl Dispatch<wl_seat::WlSeat, ()> for AppState {
      fn event(
          state: &mut Self,
          seat: &wl_seat::WlSeat,
          event: wl_seat::Event,
          _: &(),
          _: &Connection,
          qh: &QueueHandle<Self>,
      ) {
          if let wl_seat::Event::Capabilities {
              capabilities: wayland_client::WEnum::Value(caps),
          } = event
          {
              if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                  state.pointer = Some(seat.get_pointer(qh, ()));
              }
          }
      }
  }

  impl Dispatch<wl_pointer::WlPointer, ()> for AppState {
      fn event(
          state: &mut Self,
          _: &wl_pointer::WlPointer,
          event: wl_pointer::Event,
          _: &(),
          _: &Connection,
          _: &QueueHandle<Self>,
      ) {
          match event {
              wl_pointer::Event::Enter {
                  surface_x,
                  surface_y,
                  ..
              }
              | wl_pointer::Event::Motion {
                  surface_x,
                  surface_y,
                  ..
              } => {
                  state.pointer_at = Some((surface_x, surface_y));
                  let inside = state.button.contains((0.0, 0.0), surface_x, surface_y);
                  let active = state.button.states().active && inside;
                  state.update_states(inside, active);
              }
              wl_pointer::Event::Leave { .. } => {
                  state.pointer_at = None;
                  state.update_states(false, false);
              }
              wl_pointer::Event::Button {
                  state: button_state,
                  ..
              } => {
                  let pressed = matches!(
                      button_state,
                      wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed)
                  );
                  let inside = state
                      .pointer_at
                      .is_some_and(|(x, y)| state.button.contains((0.0, 0.0), x, y));
                  state.update_states(inside, pressed && inside);
              }
              _ => {}
          }
      }
  }

  impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for AppState {
      fn event(
          state: &mut Self,
          layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
          event: zwlr_layer_surface_v1::Event,
          _: &(),
          _: &Connection,
          _: &QueueHandle<Self>,
      ) {
          match event {
              zwlr_layer_surface_v1::Event::Configure {
                  serial,
                  width,
                  height,
              } => {
                  layer_surface.ack_configure(serial);
                  state.configured = Some((width, height));
                  state.dirty = true;
              }
              zwlr_layer_surface_v1::Event::Closed => state.closed = true,
              _ => {}
          }
      }
  }

  delegate_noop!(AppState: ignore wl_compositor::WlCompositor);
  delegate_noop!(AppState: ignore wl_surface::WlSurface);
  delegate_noop!(AppState: ignore wl_shm::WlShm);
  delegate_noop!(AppState: ignore wl_shm_pool::WlShmPool);
  delegate_noop!(AppState: ignore wl_buffer::WlBuffer);
  delegate_noop!(AppState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
  ```

- [ ] **Add to `ui/src/lib.rs`:** `pub mod shm;` and `pub mod wayland;`

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
  Expected: the four `shm` tests pass and everything compiles. If `Connection::flush` returns a `WaylandError` rather than `io::Error`, widen `LayerWindowError::Io` to hold it and map accordingly — keep the variant list otherwise unchanged.

- [ ] **Commit:**
  ```bash
  git add ui/src/lib.rs ui/src/shm.rs ui/src/wayland.rs
  git commit -m "$(cat <<'EOF'
  feat(ui): map the themed button as a wlr-layer-shell overlay

  Hand-rolled on wayland-client in the same shape harness/src/lib.rs
  already uses -- no smithay-client-toolkit, no calloop. Skia's raster
  Surface cannot wrap external memory, so the shm seam is an explicit
  RGBA->BGRA copy into an Argb8888 buffer; both sides are premultiplied,
  so no alpha arithmetic happens on the way through.

  Pointer enter/leave/motion/button drive :hover and :active, which
  re-runs the cascade and repaints.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 11 — The runnable binary and the harness screencopy round-trip

**Files**
- Create: `ui/src/app.rs`, `ui/tests/layer_shell_screencopy.rs`
- Modify: `ui/src/lib.rs` (add `pub mod app;`), `ui/src/bin/themed-button.rs` (replace the Task 1 stub)
- Test: `ui/tests/layer_shell_screencopy.rs`

**Interfaces**
- Consumes: `wayland::{LayerWindow, LayerWindowError}`, `css::cascade::CompiledSheet`, `css::select::{CssNode, PseudoStates}`, `text::FontStack`, `widget::button::Button`, `BUNDLED_ADWAITA_LIGHT`.
- Produces:
  ```rust
  pub enum ThemeSource { Bundled, File(PathBuf), UserPreferred }
  pub fn load_theme(source: &ThemeSource) -> String;
  pub fn run_themed_button(label: &str, classes: &[&str], theme: &ThemeSource) -> Result<(), LayerWindowError>;
  ```

### Steps

- [ ] **Write the failing test** `ui/tests/layer_shell_screencopy.rs`:
  ```rust
  //! Proves the Wayland seam: the themed button really reaches a real
  //! compositor's screen as the color the theme declares.
  //!
  //! Modelled on `compositor/tests/compat_protocols.rs`'s viewporter +
  //! screencopy test: boot the headless harness compositor, run a real
  //! client against it, capture the output, and read pixels back.

  use std::process::{Child, Command};
  use std::time::{Duration, Instant};

  use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient};

  /// Adwaita's `@accent_color`. `button.suggested-action`'s gradient runs
  /// #2c7fe3 -> #3584e4, both within 10 per channel of it, and it is far from
  /// the compositor's wallpaper (#1e1e2e) and palette foreground (#cdd6f4).
  const ACCENT: u32 = 0x0035_84E4;

  /// Kill the child on the way out however the test ends.
  struct Reaper(Child);

  impl Drop for Reaper {
      fn drop(&mut self) {
          let _ = self.0.kill();
          let _ = self.0.wait();
      }
  }

  fn pixel_at(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
      use wayland_client::protocol::wl_shm::Format;
      // Byte order per the compositor suite's own screencopy tests: Xrgb/Argb
      // are B, G, R, X in memory; Bgr888 is R, G, B despite the name.
      let (bpp, order): (usize, [usize; 3]) = match frame.format {
          Format::Xrgb8888 | Format::Argb8888 => (4, [2, 1, 0]),
          Format::Bgr888 => (3, [0, 1, 2]),
          _ => return None,
      };
      let offset = y as usize * frame.stride as usize + x as usize * bpp;
      if offset + 2 >= frame.bytes.len() {
          return None;
      }
      Some((
          frame.bytes[offset + order[0]],
          frame.bytes[offset + order[1]],
          frame.bytes[offset + order[2]],
      ))
  }

  fn close(a: u8, b: u8) -> bool {
      i32::from(a).abs_diff(i32::from(b)) <= 12
  }

  fn matches_accent(px: (u8, u8, u8)) -> bool {
      close(px.0, 0x35) && close(px.1, 0x84) && close(px.2, 0xE4)
  }

  fn count_accent_pixels(frame: &CapturedFrame) -> u32 {
      let mut count = 0;
      for y in 0..frame.height {
          for x in 0..frame.width {
              if pixel_at(frame, x, y).is_some_and(matches_accent) {
                  count += 1;
              }
          }
      }
      count
  }

  #[test]
  fn themed_button_paints_accent_blue_on_a_layer_surface() {
      let comp = Compositor::spawn();

      let child = Command::new(env!("CARGO_BIN_EXE_themed-button"))
          .env("WAYLAND_DISPLAY", &comp.socket)
          // Bundled, not the developer's own gtk.css: the assertion below is
          // Adwaita's accent blue and the test must not depend on the host.
          .env("ICEDTEA_UI_THEME", "bundled")
          .env("ICEDTEA_UI_CLASSES", "suggested-action")
          .env("ICEDTEA_UI_LABEL", "Click me")
          .spawn()
          .expect("failed to spawn themed-button");
      let _reaper = Reaper(child);

      let mut sc = ScreencopyClient::spawn(&comp.socket);
      let mut frame = sc.capture();
      let deadline = Instant::now() + Duration::from_secs(10);
      let mut accent = count_accent_pixels(&frame);
      while accent == 0 && Instant::now() < deadline {
          std::thread::sleep(Duration::from_millis(50));
          frame = sc.capture();
          accent = count_accent_pixels(&frame);
      }

      assert!(
          accent > 100,
          "only {accent} accent-blue pixels on screen: the themed button never reached the \
           compositor's output"
      );

      // And it is where the layer surface anchors it: top-left, margin 0.
      // Sample a point a few px inside the button's border box.
      let sample = pixel_at(&frame, 8, 12).expect("sample pixel inside the frame");
      assert!(
          matches_accent(sample),
          "pixel (8, 12) is {sample:?}, not the accent blue the top-left-anchored button \
           should be painting there"
      );
  }
  ```

- [ ] **Run it — expect a failure:**
  ```bash
  cargo test -p icedtea-ui --test layer_shell_screencopy
  ```
  Expected: the binary still exits 1 (`themed-button: not yet implemented`), so the loop times out and the assertion fires: `only 0 accent-blue pixels on screen`.

- [ ] **Write `ui/src/app.rs`:**
  ```rust
  //! Wiring: theme discovery plus one themed button on a layer surface.

  use std::path::PathBuf;

  use crate::css::cascade::CompiledSheet;
  use crate::css::select::{CssNode, PseudoStates};
  use crate::text::FontStack;
  use crate::wayland::{LayerWindow, LayerWindowError};
  use crate::widget::button::Button;
  use crate::BUNDLED_ADWAITA_LIGHT;

  /// Where to read the GTK4 theme from.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum ThemeSource {
      /// The vendored Adwaita copy. Hermetic; what tests use.
      Bundled,
      /// A specific `gtk.css`.
      File(PathBuf),
      /// The user's own theme if one is installed, else [`Self::Bundled`].
      UserPreferred,
  }

  /// The paths `ThemeSource::UserPreferred` probes, in order.
  fn user_theme_candidates() -> Vec<PathBuf> {
      let mut candidates = Vec::new();
      if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
          candidates.push(PathBuf::from(config).join("gtk-4.0/gtk.css"));
      } else if let Ok(home) = std::env::var("HOME") {
          candidates.push(PathBuf::from(home).join(".config/gtk-4.0/gtk.css"));
      }
      if let Ok(name) = std::env::var("GTK_THEME") {
          let name = name.split(':').next().unwrap_or(&name).to_string();
          candidates.push(PathBuf::from(format!(
              "/usr/share/themes/{name}/gtk-4.0/gtk.css"
          )));
      }
      candidates.push(PathBuf::from("/usr/share/themes/Adwaita/gtk-4.0/gtk.css"));
      candidates
  }

  /// Read the CSS for `source`, falling back to the bundled copy.
  #[must_use]
  pub fn load_theme(source: &ThemeSource) -> String {
      match source {
          ThemeSource::Bundled => BUNDLED_ADWAITA_LIGHT.to_string(),
          ThemeSource::File(path) => match std::fs::read_to_string(path) {
              Ok(css) => css,
              Err(err) => {
                  tracing::warn!(path = %path.display(), %err, "cannot read theme; using bundled Adwaita");
                  BUNDLED_ADWAITA_LIGHT.to_string()
              }
          },
          ThemeSource::UserPreferred => {
              for candidate in user_theme_candidates() {
                  if let Ok(css) = std::fs::read_to_string(&candidate) {
                      tracing::info!(path = %candidate.display(), "loaded user GTK4 theme");
                      return css;
                  }
              }
              tracing::info!("no installed GTK4 theme found; using bundled Adwaita");
              BUNDLED_ADWAITA_LIGHT.to_string()
          }
      }
  }

  /// Show one themed button on a layer surface until the compositor closes it.
  pub fn run_themed_button(
      label: &str,
      classes: &[&str],
      theme: &ThemeSource,
  ) -> Result<(), LayerWindowError> {
      let sheet = CompiledSheet::compile(&load_theme(theme));
      let fonts = FontStack::system().ok_or_else(|| {
          LayerWindowError::Io(std::io::Error::other(
              "no UI typeface found; install dejavu, liberation or noto sans",
          ))
      })?;
      let window_node = CssNode::new("window", &["background"], PseudoStates::default(), None);
      let button = Button::new(label, classes, window_node);
      let mut window = LayerWindow::open(sheet, fonts, button)?;
      window.run()
  }
  ```

- [ ] **Replace `ui/src/bin/themed-button.rs`:**
  ```rust
  //! `themed-button` — the M1 proving slice, runnable.
  //!
  //! Shows one GTK-themed button as a `zwlr_layer_shell_v1` overlay, anchored
  //! top-left. Environment:
  //!
  //! - `ICEDTEA_UI_THEME`  — `bundled`, or a path to a `gtk.css`. Default:
  //!   the user's installed GTK4 theme, falling back to the bundled Adwaita.
  //! - `ICEDTEA_UI_LABEL`   — the button's label. Default: `Click me`.
  //! - `ICEDTEA_UI_CLASSES` — comma-separated style classes, e.g.
  //!   `suggested-action`. Default: none.

  use std::path::PathBuf;

  use icedtea_ui::app::{run_themed_button, ThemeSource};

  fn main() {
      tracing_subscriber::fmt()
          .with_env_filter(
              tracing_subscriber::EnvFilter::try_from_default_env()
                  .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
          )
          .init();

      let theme = match std::env::var("ICEDTEA_UI_THEME") {
          Ok(value) if value == "bundled" => ThemeSource::Bundled,
          Ok(value) if !value.is_empty() => ThemeSource::File(PathBuf::from(value)),
          _ => ThemeSource::UserPreferred,
      };
      let label = std::env::var("ICEDTEA_UI_LABEL").unwrap_or_else(|_| "Click me".to_string());
      let classes_raw = std::env::var("ICEDTEA_UI_CLASSES").unwrap_or_default();
      let classes: Vec<&str> = classes_raw
          .split(',')
          .map(str::trim)
          .filter(|c| !c.is_empty())
          .collect();

      if let Err(err) = run_themed_button(&label, &classes, &theme) {
          tracing::error!(%err, "themed-button failed");
          std::process::exit(1);
      }
  }
  ```

- [ ] **Add to `ui/src/lib.rs`:** `pub mod app;`

- [ ] **Add the test's `wayland-client` dev-dependency** to `ui/Cargo.toml` — it is already a normal dependency, so nothing to add; confirm `cargo tree -p icedtea-ui -i wayland-client` shows one version only.

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```
  Expected: `themed_button_paints_accent_blue_on_a_layer_surface ... ok`. If it times out, run the binary against the harness by hand and read its `tracing` output — `RUST_LOG=debug WAYLAND_DISPLAY=<socket> ICEDTEA_UI_THEME=bundled ./target/debug/themed-button` — before touching the assertion.

- [ ] **Commit:**
  ```bash
  git add ui/src/lib.rs ui/src/app.rs ui/src/bin/themed-button.rs ui/tests/layer_shell_screencopy.rs
  git commit -m "$(cat <<'EOF'
  feat(ui): run the themed button under the harness compositor

  A runnable `themed-button` binary plus a screencopy round-trip: boot the
  headless harness compositor, run the real client against it, capture the
  output and assert Adwaita's accent blue is on screen where the
  top-left-anchored layer surface puts it. Modelled on the compositor
  suite's viewporter/screencopy test.

  Runtime prefers the user's own gtk.css; ICEDTEA_UI_THEME=bundled keeps
  the test hermetic.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 12 — Documentation and spec status

**Files**
- Create: `ui/README.md`
- Modify: `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
- Test: none (documentation only; the gates still run).

**Interfaces**
- Consumes: everything above.
- Produces: no code.

### Steps

- [ ] **Write `ui/README.md`:**
  ```markdown
  # `icedtea-ui`

  A pure-Rust, GTK4-theme-compatible widget layer: no `gtk4`, `gio`, `glib`,
  `pango`, `cairo` or `gdk`, and no Smithay. See
  [the design spec](../docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md).

  ## What M1 covers

  One themed `button`, end to end:

  | Layer | Crate |
  |---|---|
  | Wayland + layer shell | `wayland-client`, `wayland-protocols-wlr` (hand-rolled, no sctk/calloop) |
  | 2D paint | `skia-rs-safe` (pure Rust) |
  | Text | `skia-rs-text` — `Typeface::from_data` + `rustybuzz` shaping (**not** `cosmic-text`) |
  | Layout | `taffy` |
  | CSS parse / match | Servo's `cssparser` + `selectors` |
  | Widget | bespoke — `CssNode` wears GTK's node identity |

  ## Running it

  ```bash
  cargo run -p icedtea-ui --bin themed-button
  ```

  Environment:

  - `ICEDTEA_UI_THEME` — `bundled`, or a path to a `gtk.css`. Default: the
    user's installed GTK4 theme (`$XDG_CONFIG_HOME/gtk-4.0/gtk.css`, then
    `/usr/share/themes/$GTK_THEME/gtk-4.0/gtk.css`, then
    `/usr/share/themes/Adwaita/gtk-4.0/gtk.css`), falling back to the
    bundled copy.
  - `ICEDTEA_UI_LABEL` — the button's label.
  - `ICEDTEA_UI_CLASSES` — comma-separated style classes, e.g.
    `suggested-action`.

  ## Tests

  ```bash
  cargo test -p icedtea-ui
  ```

  - `tests/themed_button_offscreen.rs` is the **load-bearing gate**: computed
    values equal Adwaita's resolved values, the centre pixel equals the color
    the theme declares, a pixel outside `border-radius: 5px` is transparent,
    and toggling `:hover`/`:active` changes both. No compositor needed.
  - `tests/layer_shell_screencopy.rs` proves the Wayland seam against the
    harness compositor.

  ## Deliberately not covered by M1

  More than one widget; full selector and `-gtk-*` property coverage;
  box-shadows, radial gradients, `url()` images; GTK's `alpha()`/`shade()`/
  `mix()` color functions and CSS relative color syntax; icons; animations
  and transitions; accessibility; input methods; fractional scale; the app
  framework. See the spec's M2–M6.

  ## Vendored files

  `themes/adwaita-light.css` is GTK 4's default light theme, redistributed
  under the LGPL-2.1-or-later. See [`themes/README.md`](themes/README.md).
  ```

- [ ] **Update the spec's status block.** In `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`, replace:
  ```markdown
  **Status:** proposed — awaiting user review
  ```
  with:
  ```markdown
  **Status:** M1 implemented (see `docs/superpowers/plans/2026-08-25-pure-rust-gtk-m1-proving-slice.md`); M2–M6 proposed
  ```

- [ ] **Record M1's two resolved risks.** In the spec's `## Risks` section, replace the `**Text stack decision**` bullet with:
  ```markdown
  - ~~**Text stack decision**~~ — **resolved in M1: `skia-rs-text`.** It loads
    a TTF/OTF from raw bytes, shapes via `rustybuzz` with real `hmtx`
    advances, and rasterizes glyph outlines through `Canvas::draw_text_blob`,
    so the whole load/shape/measure/draw path is one crate. `cosmic-text` is
    not used.
  ```
  and replace the `**`skia-rs` build/linking (C++ Skia)**` bullet with:
  ```markdown
  - ~~**`skia-rs` build/linking**~~ — **resolved in M1: not a risk.**
    `skia-rs` is a pure-Rust reimplementation, not a binding; there is no C++
    build and no linking step.
  ```

- [ ] **Run the gates:**
  ```bash
  cargo test -p icedtea-ui
  cargo clippy -p icedtea-ui --all-targets -- -D warnings
  cargo fmt --all --check
  ```

- [ ] **Commit:**
  ```bash
  git add ui/README.md docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md
  git commit -m "$(cat <<'EOF'
  docs(ui): document the M1 slice and record its resolved risks

  Two of the spec's risks close with M1: the text stack is skia-rs-text
  (not cosmic-text), and skia-rs turns out to be a pure-Rust
  reimplementation rather than a C++ binding, so there is no build or
  linking risk at all.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Self-Review

### Spec coverage — every M1 bullet maps to a task

| Spec requirement (§Milestone 1) | Task |
|---|---|
| New crate `ui/` (`icedtea-ui`) in the workspace, pulling `wayland-client`, `wayland-protocols`, `wayland-protocols-wlr`, `skia-rs`, `taffy`, `cssparser`, `selectors`; no Smithay | 1 |
| Narrow path 1 — `wlr-layer-shell` surface, `wl_shm` buffer in `Argb8888`, damage/commit driven by a dispatch loop, reusing the repo's existing `wayland-client` client shape | 10 |
| Narrow path 2 — a raster Skia surface whose pixels reach the shm buffer; the stride/format/premultiply seam stated explicitly | 10 (`shm.rs`; the "cannot wrap external memory" finding is in the Architecture paragraph and `shm.rs`'s module doc) |
| Narrow path 3a — load a GTK4 `gtk.css`: Adwaita from disk when present, else a bundled copy so the test is hermetic | 1 (vendoring), 11 (`ThemeSource::UserPreferred` probe + fallback) |
| Narrow path 3b — parse with `cssparser` into (selector-list, declaration-block) rules | 2 |
| Narrow path 3c — resolve `@define-color` into a color table | 3 |
| Narrow path 3d — model the button as a CSS node (name, ancestor chain, classes, `:hover`/`:active`) and implement `selectors`' `Element` | 4 |
| Narrow path 3e — cascade by specificity/order; compute `background(-color)`, `color`, `border-{width,color,radius}`, `padding`, `min-width`/`min-height`, `font-*` | 5 (cascade), 6 (computed) |
| Narrow path 4a — widget node identity + interaction state | 9 (`widget/button.rs`) |
| Narrow path 4b — `taffy` computes the allocation from padding + intrinsic shaped text size | 8 |
| Narrow path 4c — Skia paints the rounded-rect background + border per the computed style | 9 (`paint.rs`) |
| Narrow path 4d — the label is shaped and drawn via Skia glyph runs | 7 (shape/measure), 9 (`draw_text_blob` + `the_label_is_actually_drawn`) |
| Narrow path 4e — pointer enter/leave/press toggles `:hover`/`:active` and re-runs cascade+paint | 10 (`wl_pointer` handlers → `AppState::update_states` → `Button::set_states` → `repaint`) |
| Success criterion — offscreen render test asserting computed `background`, `border-radius`, `color` equal the theme's resolved values | 9 (`adwaita_button_computed_style_and_pixels_match_the_theme`, section 1) |
| Success criterion — centre pixel equals the computed background | 9 (sections 2/3/4: exactly `#dad6d2` for `:active`, exactly `#e8e6e3` for `:hover`, and for the normal gradient the flat pre-stop band `#f6f5f4` plus a strict between-the-stops bound at the centre) |
| Success criterion — a pixel just outside the corner radius is transparent | 9 (`pixel(&surface, 0, 0).alpha() == 0`, asserted for both the normal and `:active` renders) |
| Success criterion — toggling `:hover` changes the computed background and the centre pixel | 9 (section 3: `assert_ne!` on both the `Background` and the painted pixel) |
| Success criterion — runnable binary showing the button as a layer surface, with a harness screencopy check | 11 |
| "The offscreen test is the load-bearing gate" | Named in Global Constraints and gated in Task 9 |
| Text-stack decision made in M1 | Architecture paragraph + Task 7 + Task 12 (spec risk closed) |
| YAGNI exclusions honoured (one widget, no icons, no animations, no app framework, no migration) | No task creates a second widget, an icon loader, a transition engine, a reactivity model, or touches `shell`/`settings`/`clipboard` |

### Placeholder scan

Searched the plan for `TBD`, `TODO`, `FIXME`, `...`, "similar to Task", "add error handling", "as appropriate", "etc.":

- No `TBD`/`TODO`/`FIXME` appears in any code block.
- No task says "similar to Task N" — every code block is written out in full, including the repeated `fn button(classes, states)` fixtures in Tasks 5, 6 and 9, which are deliberately duplicated rather than cross-referenced.
- Error handling is concrete everywhere: `LayerWindowError` enumerates its four cases with `Display` and `Error` impls; parse failures log via `tracing::debug!` and skip; `FontStack::system()` returns `Option` and its `None` is converted to a named `io::Error` message in `app.rs`.
- The only two remaining conditional instructions are verification steps with a stated criterion and a stated remedy, not deferred work: Task 1's line-count check on the extracted CSS (stop and re-derive if GTK differs), Task 3's `table.len() == 29` check (print the unresolved names and correct the constant), and Task 10's `Connection::flush` error-type note (widen the `Io` variant). Each names exactly what to do.
- Every referenced type is defined in this plan or in a crate whose real signature was read while writing it: `Surface::new_raster_n32_premul`, `Surface::pixels`, `Surface::pixel_buffer`, `PixelBuffer::get_pixel`, `Canvas::{clear, draw_rect, draw_round_rect, draw_path, clip_path, save, restore_to_count, draw_text_blob}`, `ClipOp::Intersect`, `Paint::{new, set_color32, set_style, set_stroke_width, set_anti_alias}`, `Style::{Fill, Stroke}`, `Color::{from_argb, from_css, alpha, red, green, blue, TRANSPARENT, BLACK}`, `Rect::{new, from_xywh}`, `PathBuilder::{new, add_round_rect, build}`, `Typeface::{from_data, family_name}`, `Font::{new, metrics, measure_text, size}`, `FontMetrics::{ascent, descent, line_height}`, `Shaper::{new, shape_auto}`, `ShapedRun::{glyphs, font, width}`, `ShapedGlyph::{glyph_id, x_advance, x_offset, y_offset}`, `TextBlobBuilder::{new, add_positioned_run, build}`, `TextBlob::runs`, `GlyphRun::{glyphs, positions, font, origin}`; `cssparser::{Parser, ParserInput, ParserState, StyleSheetParser, RuleBodyParser, RuleBodyItemParser, DeclarationParser, QualifiedRuleParser, AtRuleParser, CowRcStr, ToCss, SourceLocation}`; `selectors::{SelectorImpl, SelectorList, Element, OpaqueElement}`, `selectors::parser::{Parser, ParseRelative, SelectorParseErrorKind, NonTSPseudoClass, PseudoElement, Selector::specificity}`, `selectors::matching::{matches_selector_list, MatchingContext, ElementSelectorFlags}`, `selectors::context::{MatchingMode, QuirksMode, NeedsSelectorFlags, MatchingForInvalidation, SelectorCaches}`, `selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint}`, `selectors::bloom::BloomFilter`; `taffy::prelude::{TaffyTree, Style, Size, Display, AvailableSpace, AlignItems, JustifyContent, length}`, `taffy::geometry::Rect`, `taffy::tree::Layout::{size, location}`; `precomputed_hash::PrecomputedHash`; `icedtea_harness::{Compositor, ScreencopyClient, CapturedFrame}`.

### Type consistency across tasks

- `Color` is `skia_rs_safe::core::Color` (a `#[repr(transparent)]` `u32` in `0xAARRGGBB`) everywhere — in `ColorTable` (T3), `ComputedStyle`/`Background` (T6), `Paint::set_color32` (T9) and every pixel assertion (T9). `PixelBuffer::get_pixel` reassembles the same `0xAARRGGBB` layout from the buffer's physical `R,G,B,A` bytes, so `pixel(...) == Color(0xFFDAD6D2)` compares like with like.
- `Declaration` is produced by T2 and consumed unchanged by T5's `CompiledRule` and T6's `from_declarations` (which takes `HashMap<String, String>`, exactly `cascade`'s return type).
- `CssNode` is created in T4, stored in T9's `Button`, mutated only through `with_states`, and never leaks its `Rc` — `Element::parent_element` returning `Option<Self>` is the reason it is `Rc`-shaped, and that requirement is stated where the type is defined.
- `PseudoStates` is one `Copy` struct defined in T4 and used by T5's tests, T6's tests, T9's `Button::set_states`, T10's `AppState::update_states` and T11's `app.rs`.
- `ComputedStyle` is defined once (T6) and read by T8 (`layout_button`), T9 (`paint_button`, `Button::style`) and T9's test. Its `padding` is `[top, right, bottom, left]` in every one of them; T8's test asserts `label_x == border + padding[3]` and `label_y == border + padding[0]`, matching that order.
- `Allocation` (T8) is returned by `layout_button`, cached on `Button`, and consumed by `paint_button` and both tests; `width`/`height` are always the **border box**, which is what T10 uses to size the layer surface and the shm buffer.
- `TextMetrics` (T7) is produced by `FontStack::measure` and consumed by T8's `layout_button` (`width`, `line_height`) and T9's `paint_button` (`ascent`, for the baseline). `ascent` is sign-flipped once, in `FontStack::measure`, so every downstream user sees it positive-upward — stated in that function's doc.
- `FontStack` is created once per process (T9's test fixture, T11's `run_themed_button`) and passed by reference into `Button::restyle`, `Button::render` and `paint_button`; it is never cloned, so the loaded `Arc<Typeface>` is shared.
- `AppState` is `wayland.rs`'s `Dispatch` state and also the `QueueHandle` parameter of `ShmBuffer::new` in `shm.rs`; `shm.rs` imports it from `crate::wayland`, and `wayland.rs` imports `ShmBuffer` from `crate::shm` — a module cycle Rust permits, and the only coupling between the two.
- `CompiledSheet` is built once (T5) and borrowed by `ComputedStyle::resolve`, `Button::restyle`/`set_states` and `AppState`; it owns its `ColorTable`, so no caller has to thread colors separately.
