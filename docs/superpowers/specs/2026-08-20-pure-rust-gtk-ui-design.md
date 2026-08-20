# Pure-Rust, GTK-themed Desktop UI — Design

**Date:** 2026-08-20
**Status:** proposed — awaiting user review
**Branch:** `rebuild/pure-rust-gtk`

## Goal

Re-platform icedtea's **UI layer** — the `shell` (bar/panels), `settings`, and the
clipboard's GTK shell — off `gtk4-rs` and the GNOME platform
(GObject/GLib/Gio/Pango/Cairo/GDK) onto a **pure-Rust, crate-reuse-heavy stack**,
while remaining **compatible with the user's installed GTK4 CSS themes** so the
desktop honors whatever GTK theme they run. The `wlr`/wlroots **compositor is
unchanged**.

"Pure-Rust" here means **dropping the GTK/GNOME/Gio stack and maximizing crate
reuse**, not avoiding C absolutely: the compositor keeps wlroots (C), and 2D
paint uses Skia (C++) via the owner's `skia-rs` crate. The point is to leave the
GNOME toolkit, not to purge every native dependency.

## Decisions (settled during brainstorming)

1. **Scope: UI layer only.** Rebuild `shell`, `settings`, clipboard-shell. Keep
   the `wlr` compositor and the `config`/`contract`/`harness` crates. Reuse the
   existing **pure cores** (GTK-free Model/apply/geometry/outputs-client/
   key-combo logic).
2. **Drop:** `gtk4`, `gio`, `glib`, `gobject`, `pango`, `cairo`, `gdk`.
3. **Theming: GTK4 theme-FILE compatible, BROAD fidelity.** Parse and apply the
   user's installed GTK4 themes (`~/.config/gtk-4.0/gtk.css`,
   `/usr/share/themes/<Name>/gtk-4.0/`), targeting near-complete GTK4 CSS so most
   third-party themes "just work". Reached iteratively (see decomposition).
4. **Maximize crate reuse.** Prefer proven crates for every layer except the one
   piece that must be bespoke (the widget toolkit — see below).
5. **2D paint: `skia-rs`** (owned by the user). Skia's gradients, box-shadows,
   blur, rounded-rect borders, and text are exactly what broad GTK theming needs.
6. **CSS core: Servo `cssparser` + `selectors`.** Reimplementing CSS parsing and
   selector matching correctly is the riskiest, least-differentiated work;
   Servo's crates (the Stylo/Firefox lineage) are the pure-Rust standard.
7. **Widget toolkit: clean-room**, built directly on `taffy` + `skia-rs` + the
   GTK-CSS engine from day one (no reuse-evaluation detour).
8. **First build: a thin vertical proving slice**, not a breadth-first
   foundation — prove the whole pipeline through one widget before scaling.

## Why theme-file compatibility forces a bespoke widget layer

For a real theme's selectors to match — `headerbar > button.suggested-action:hover`,
`@define-color accent_bg_color …`, `-gtk-*` properties — our widgets must present
the **same CSS node tree GTK does**: node names (`window`, `headerbar`, `button`,
`label`, `entry`, …), style classes (`.suggested-action`, `.flat`, `.title`, …),
and pseudo-classes (`:hover`, `:active`, `:checked`, `:disabled`, `:backdrop`, …).
No existing Rust widget crate models GTK's CSS node identity, so adopting one and
retrofitting GTK-theme compatibility would amount to rewriting it. Hence the
widget layer is the single irreducible bespoke component; **everything above and
below it is reused crates.**

## Architecture (layered stack)

| Layer | Crate(s) | Replaces | Build vs reuse |
|---|---|---|---|
| Wayland + layer-shell | `smithay-client-toolkit`, `wayland-protocols-wlr`, `calloop` | GDK + Gio Wayland backend | reuse |
| 2D paint | **`skia-rs`** (owned) | Cairo | reuse |
| Text | `cosmic-text` (shape/layout) rasterized via Skia — Skia can also shape via HarfBuzz, so this may collapse (resolved in M1) | Pango | reuse |
| Layout | `taffy` (flex/grid/block), wrapped to express GTK's measure→allocate box model | GTK layout managers | reuse |
| CSS core | Servo `cssparser` + `selectors` | GtkCssProvider parse/match | reuse |
| Accessibility | `accesskit` (later milestone) | ATK / AT-SPI | reuse |
| Icons | freedesktop icon-theme loader + Skia recolor for symbolics (later) | GtkIconTheme | reuse |
| **Widget toolkit** | **bespoke** — widgets wearing GTK CSS node identity | GTK widgets + GObject/signals | **build** |
| App logic | existing **pure cores** | (kept) | reuse |

## Decomposition (this is a program, not one spec)

The work spans a CSS engine, a windowing/paint substrate, a widget toolkit, icon
theming, and three app migrations — multiple independent subsystems. It is
decomposed into sub-projects that each ship something testable. **Milestones ≥ 2
get their own spec** when reached; this document details M1 and sketches the rest.

1. **M1 — Proving slice** (detailed below): one themed button end-to-end.
2. **M2 — GTK-CSS engine breadth**: full parse + `@define-color` + selector/
   combinator coverage + cascade + specificity + the `-gtk-*` property set +
   gradients/shadows, tested against real theme files on abstract node trees.
3. **M3 — Widget toolkit breadth**: the retained widget tree + event/focus model
   + a core widget set (window, headerbar, button, label, box, grid, entry,
   switch, checkbutton, dropdown, scrolledwindow, listview/row, stack,
   stackswitcher, spinbutton, colorbutton), each with GTK-matching nodes.
4. **M4 — Icon & asset theming**: freedesktop icon themes, symbolic recolor,
   cursor themes, HiDPI/fractional scale assets.
5. **M5 — App migrations**: rebuild `settings`, then `shell`/bar, then the
   clipboard shell on the toolkit, each reusing its pure core and dropping
   `gtk4`. Ordered settings → shell → clipboard (settings is the richest widget
   exerciser; shell adds layer-shell surface roles; clipboard is smallest).
6. **M6 — Polish**: CSS transitions/animations, `accesskit`, input methods,
   drag-and-drop, settings-portal-ish integration.

---

## Milestone 1 — "one themed button, end-to-end"

**Goal:** prove the entire pipeline through one narrow path — a `wlr-layer-shell`
window rendering a single **button** whose appearance is driven by a *real*
loaded GTK4 theme — so every seam (Wayland → CSS → widget → Skia paint → text) is
exercised before we scale breadth. It front-loads the two riskiest unknowns:
(a) does Servo's `selectors` cleanly match GTK's node model, and (b) does
Skia-on-`wl_shm` paint a themed widget crisply.

**New crate** `ui/` (e.g. `icedtea-ui`), added to the workspace, pulling:
`smithay-client-toolkit`, `skia-rs`, `cosmic-text`, `taffy`, `cssparser`,
`selectors`, `calloop`.

### The narrow path

1. **Window** — an sctk `wlr-layer-shell` surface (a small overlay), a `wl_shm`
   buffer in a `BGRA8888`/`Argb8888` format, damage/commit driven by `calloop`.
   Reuses the project's existing sctk / `wayland-protocols-wlr` experience
   (clipboard, harness).
2. **Skia surface** — an `SkSurface` (raster) backed by the shm buffer's pixel
   memory; paint into it, then commit the buffer. Establishes the Skia↔`wl_shm`
   seam (stride, format, premultiplied alpha).
3. **CSS engine (minimal, enough for the slice)**:
   - Load a GTK4 `gtk.css` (Adwaita) — from `/usr/share/themes/Adwaita/gtk-4.0/
     gtk.css` when present, else a bundled copy so the test is hermetic.
   - Parse with `cssparser` into (selector-list, declaration-block) rules.
   - Resolve `@define-color` named colors into a color table.
   - Model the button as a **CSS node**: element name `button`, ancestor chain
     `window > … > button`, style classes, and pseudo-class state
     (`:hover`, `:active`). Implement the `selectors` crate's `Element` trait for
     our node so its matcher drives selection.
   - Cascade matched declarations by specificity/order; compute the properties
     the slice needs: `background(-color)`, `color`, `border-{width,color,radius}`,
     `padding`, `min-width`/`min-height`, `font-*`.
4. **Button widget** — node identity + interaction state; `taffy` computes the
   allocation from padding + intrinsic (shaped) text size; Skia paints the
   rounded-rect background + border per the computed style; `cosmic-text` shapes
   the label, drawn via Skia glyph runs; pointer enter/leave/press toggles
   `:hover`/`:active` and re-runs cascade+paint.

### Data flow

theme `gtk.css` → parsed stylesheet + `@define-color` table → (button node +
state) → `selectors` match → specificity cascade → computed style → `taffy`
layout + Skia paint + `cosmic-text` → pixels → `wl_shm` buffer → layer surface
commit.

### Success criteria & load-bearing test

- **Offscreen render test (non-vacuous, no compositor needed):** load Adwaita,
  build the button node, and assert the *computed* `background`, `border-radius`,
  and `color` equal the theme's resolved values (e.g. the button background
  resolves through Adwaita's named colors). Render to an offscreen `SkSurface`
  and assert pixels: the button's center pixel equals the computed background
  color, and a pixel just outside the corner radius is transparent. Toggling
  `:hover` changes the computed background and the center pixel. This proves
  CSS → cascade → Skia paint end-to-end, and fails if any seam is wrong.
- **Runnable binary:** shows the themed button as a layer surface under the
  running icedtea compositor (manual/visual, or a harness screencopy check).
- The offscreen test is the load-bearing gate; the layer-shell binary proves the
  Wayland seam.

### Deliberately excluded (YAGNI for M1)

More than one widget; full selector / `-gtk-*` coverage; icons; animations; the
app framework / reactivity model; any real app migration. Just enough of each
layer to prove the seam.

---

## Testing strategy (program-wide)

- **CSS engine:** unit tests parsing real theme files (Adwaita, Yaru) and
  asserting computed styles for hand-built node trees — the engine is decoupled
  from widgets and testable on abstract nodes.
- **Rendering:** offscreen Skia render tests with pixel assertions (the M1
  pattern), so CSS→paint is verified without a compositor.
- **Wayland integration:** run the UI as a layer-shell/xdg client against the
  project's **harness compositor** (already used for screencopy/GTK tests),
  asserting via screencopy where a visual check is load-bearing.
- **App migrations:** each migrated app keeps its existing pure-core tests; the
  new view is covered by render tests + a harness round-trip.

## Risks

- **`selectors`/`cssparser` fit to GTK's node model** — GTK CSS differs from web
  CSS in its node tree and `-gtk-*` extensions. M1's button spike resolves
  whether Servo's matcher adapts cleanly or needs a shim.
- **`skia-rs` build/linking** (C++ Skia) — build-system and packaging cost;
  validate the build early in M1.
- **Text stack decision** — `cosmic-text` shaping + Skia raster vs Skia's own
  HarfBuzz shaping; M1 picks one and the rest follows.
- **GTK CSS is a moving target** — broad fidelity is iterative; M2 defines the
  coverage bar and tracks against specific GTK releases.
- **Parity gaps** — input methods, drag-and-drop, fractional scale, cursor
  themes, a11y are deferred to M4/M6 and are real work.
- **Program size** — large; mitigated by independently-testable milestones and
  by reusing the existing GTK-free pure cores so app *logic* is not rewritten.

## Open questions (deferred to later milestones)

- The app framework / reactivity model: retained tree with explicit updates, or a
  reactive layer over it? (Decide at M3.)
- Icon theme + symbolic recolor details (M4).
- Settings-portal / cursor-theme / fractional-scale integration (M6).
