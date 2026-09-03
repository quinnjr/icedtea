# Pure-Rust GTK-themed UI — M5 Part 2: settings Appearance + Behavior pages, portal wallpaper picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `icedtea-settings`' Appearance and Behavior pages off GTK onto `icedtea-ui`'s reactive toolkit, and give the wallpaper field a real `org.freedesktop.portal.FileChooser.OpenFile` picker with an always-authoritative validated `Entry` beside it.

**Architecture:** Both pages become pure `fn view(&SettingsModel) -> View<Msg>` functions composed into P1's `Stack`; every user edit is a `Msg` folded by `app::update` into `model.working`, so there is no populate/write-back guard left to keep. The portal picker is a `crossbeam-channel`-fed worker thread that speaks blocking zbus, answers on the P0 inbox as `Msg::WallpaperChosen`/`Msg::WallpaperPickerFailed`, and is reached from `update` only through `Cmd::Task`.

**Tech Stack:** Rust edition 2024 (rust-version 1.94), `icedtea-ui` (M3 + M5 P0), `icedtea-config`, `icedtea-contract`, `zbus` 5 (workspace), `crossbeam-channel` (workspace), `redb` (workspace), `tempfile` 3, `icedtea-harness` (dev), `skia-rs-safe` 0.4.0 via `icedtea-ui`.

**Spec:** `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md` (§2 D1–D9 and the eleven inherited decisions, §5.1, §5.4, §7) together with the frozen interface contract `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§2.2, §2.3, §2.6, §5 "P2"). Context: `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`, `docs/superpowers/specs/2026-08-27-pure-rust-gtk-m3-widget-toolkit-design.md`, `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§3 interfaces, §10 P3-A…P8-D75, §11 E1–E21).

---

## Contract deviations

Every line here is a departure from `2026-09-03-m5-part0-contract.md` that this part makes deliberately. P6 copies them into that file's §6 (Task 11 does the append).

- **P2-D1 — P2 creates `settings/src/ipc/portal.rs` and completes `WorkerHandles`.** Contract §5 gives `ipc/portal.rs` to P2 (new file), but §2.4 is headed `(P1)` and shows a `WorkerHandles { reload, portal }` with `choose_wallpaper`. P1 resolves that split its own way (**P1-D7**): it ships the `reload` half only — no `portal` field, no `choose_wallpaper`, no `ipc/portal.rs`, no `PortalRequest` — and P1-D7 is explicitly *binding on P2* to land the other half. Ruling, reconciled with P1-D7: P2 **creates** the file (there is no P1 stub to replace), adds the `portal` field to `WorkerHandles`, adds `WorkerHandles::choose_wallpaper`, extends `ipc::spawn` to start the portal worker beside the reload worker, and extends P1's `ipc::handles_for_test` to hand back the portal receiver in the same tuple shape. `PortalRequest`, `pub fn spawn(rx, tx) -> JoinHandle<()>` and `choose_wallpaper` use contract §2.4's signatures exactly, so §2.4 is the union across P1+P2 and nothing P1 shipped changes. (Consistency-check ruling E5.)
- **P2-D2 — `settings/tests/support/mod.rs` is shared with P1.** The contract names no settings test-support module, yet P1's gate `the_first_toplevel_app_run_paints_and_navigates` and every P2–P4 gate need one. Ruling: Task 9 creates it if absent and otherwise adds only the items it does not already contain; the signatures in Task 9 are normative for P3/P4.
- **P2-D3 — three additive helpers in `pages/appearance.rs`.** `WALLPAPER_EXTENSIONS`, `validate_wallpaper` and `spin_px` are not in contract §2.1's extraction table. They are additive, `pub`, and named here so P3/P4 can rely on them.
- **P2-D4 — `a_colour_pick_changes_the_swatch` moves from P4 to P2.** Contract §2.8 lists it in P4's gate list, but §5 forbids P4 from touching another page. Ruling: it is an Appearance-page gate and lands in `settings/tests/appearance.rs` (Task 11); P4's list loses it.
- **P2-D5 — new integration binary `settings/tests/portal_worker.rs`.** Not in contract §5's P2 "Owns" list. It exists because the portal degradation proof mutates process-global environment (`DBUS_SESSION_BUS_ADDRESS`) and must therefore own its process.
- **P2-D6 — `color_dialog_button(rgba)`, not `color_dialog_button(hex_to_packed(&hex))`.** Contract §2.6's table passes a packed `f64`; the shipped M3 builder is `color_dialog_button(rgba: Rgba)` (`ui/src/widgets/color_dialog.rs:34`), which packs internally. Superseded in practice by P2-D11 below, which drops the widget; `hex_to_packed`/`packed_to_hex` remain the model↔handler currency.
- **P2-D7 — the settings binary honours `$ICEDTEA_UI_THEME`.** Needed so a gate can run the same page in three themes, exactly as `gallery --theme` does. Task 9 adds it to `settings/src/main.rs` if P1 did not. **P2 owns this knob for the rest of M5**: P3 restates it as P3-D6 and P4's gates rely on it, so P2's shipped reading is the one they consume. Follow P3-D6's spelling — the variable names a **path to a complete base theme file** (`ui/src/bin/window-probe.rs:41`'s convention), with `settings/style.css` layered over it as the app origin; the three-way `light`/`dark`/`hc` choice is made by the gate writing `icedtea_ui::BUNDLED_ADWAITA_{LIGHT,DARK,HC}` to a temp file. (Consistency-check ruling E3.)
- **P2-D8 — the settings binary honours `ICEDTEA_SETTINGS_PAGE`.** A page name (`"appearance"`, `"behavior"`, …) selecting the initially visible stack page, so a rest-state gate does not have to synthesise a switcher click. Same role as `gallery --widget`. Task 9 adds it.
- **P2-D9 — one contract gate name becomes three per-theme test functions.** `appearance_page_paints_every_probe_point_at_rest` ships as `…_in_the_light_theme` / `…_in_the_dark_theme` / `…_in_the_high_contrast_theme`, matching `ui/tests/gallery_gate.rs`'s convention; likewise for `behavior_page_…`.
- **P2-D10 — `snap_gap` is a Behavior-page control writing `working.appearance.snap_gap`.** Spec §5.4 and contract §2.3's id list put it on Behavior; the GTK page had it on Appearance and the config field stays where it is (`icedtea_contract::Appearance::snap_gap`). No config change.
- **P2-D11 — colour swatches are `button_from(drawing_area(..))`, not `color_dialog_button`/`color_dialog`.** Both of those controllers hit-test through `local_rect(cx.tree, cx.node, &subnode)` against nodes the controller appends itself, and such nodes get **no taffy allocation** in a live tree (`ui/src/widgets/scrollbar.rs:309-317` and `ui/src/widgets/mod.rs:174-180` both state it), so neither the button's toggle nor a palette swatch's pick is reachable from a real pointer — a live "pick a colour" gate could not pass. Ruling: the page paints each swatch with `drawing_area` (P0's M5-D6 three-argument `Prop::Draw`) inside a `button_from`, which is an ordinary clickable `View` child with a real id, a real allocation and a real probe point. Adds `SettingsModel::color_picker: Option<ColorSlot>` and `Msg::{ColorPickerOpened(ColorSlot), ColorPickerClosed}`; `Msg::{BackgroundPicked, ForegroundPicked, AccentPicked}(f64)` keep the contract's shape and are now emitted by the palette buttons.
- **P2-D12 — P2 edits `settings/src/app.rs` and `settings/src/main.rs`.** Contract §5 lists P2's "Owns" as the two page modules, `ipc/portal.rs` and the tests, and forbids only "other pages" and `ui/`. §2.2 puts `update` in `app.rs` and §2.3 puts the root `view` there, so the Appearance/Behavior arm groups and the two env knobs above are necessarily edits to `app.rs`/`main.rs`. P2 touches no other arm group.

---

## Global Constraints

Copied verbatim from the spec and the contract; every task's requirements implicitly include this section.

- **Pins.** `wayland-client` 0.31, `wayland-protocols-wlr` 0.3 (`features = ["client"]`), `zbus` workspace (5.18.0), `rustix` 1, `crossbeam-channel` workspace (0.5), `redb` workspace, `tempfile` 3, `skia-rs-safe` 0.4.0, `taffy` 0.14, `xkbcommon` 0.9, `wlr` 0.20.29.
- **No `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gobject`, `gdk`, `gdk4`, `pango` or `cairo`** in a migrated app — neither in `settings/Cargo.toml` nor in any `use` inside `settings/src` or `settings/tests`.
- **Edition 2024, `rust-version = "1.94"`.**
- **Gates per crate:** `cargo test -p icedtea-settings`; `cargo clippy -p icedtea-settings --all-targets -- -D warnings`; `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`; `cargo fmt --all --check`.
- **Every M1/M2/M3 gate stays green, unchanged:** `themed_button_offscreen`, `adwaita_coverage`, `gtk4_property_reference`, `layer_shell_screencopy`, `transition_screencopy`, `window_events`, `counter_app`, `node_trees`, `widget_pixels`, `reconcile_props`, `gallery_gate` (light/dark/hc), `interaction_gate` (16/16).
- **Parts execute strictly in order P0 → P1 → P2 → P3 → P4 → P5 → P6**, each based on the previous part's head, so P2 may consume everything P0 and P1 produced.
- **Settings swaps `gtk4` out in place (spec D3).** There is no second binary and no feature flag; after P1 the GTK settings binary no longer exists, and P2 never re-introduces it.
- **`settings/tests/live_apply.rs` keeps spawning the real `icedtea-compositor` binary** (inherited decision 11) — never `icedtea_harness::Compositor`. P2 does not touch that file.
- **`Msg` is `Send`**, every payload owned or `Arc`, never `Rc` (M5-D2 §1). `Cmd::Task`'s body must not block (M5-D3).
- **Untrusted input never panics** — portal replies, D-Bus signal bodies, hand-edited config values, file paths: a malformed value is dropped, logged once, and replaced by the fallback.
- **Every load-bearing test records a mutation check:** break the code the test claims to cover, confirm the test fails, restore.
- **Commit trailer**, on every commit in this plan:
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  ```

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `settings/src/pages/appearance.rs` | Modify (P1 left the extracted pure half) | `WALLPAPER_EXTENSIONS`, `validate_wallpaper`, `spin_px`, `swatch`, `palette_button`, `pub fn view(&SettingsModel) -> View<Msg>`. Keeps P1's `hex_to_rgba`/`rgba_to_hex`/`hex_to_packed`/`packed_to_hex` untouched. |
| `settings/src/pages/behavior.rs` | Rewrite | Three `switch` rows plus the snap-gap `spin_button`; `pub fn view(&SettingsModel) -> View<Msg>`. |
| `settings/src/ipc/portal.rs` | Create / replace P1 stub | `PortalRequest`, `spawn`, `open_file`, and the pure helpers `request_object_path`, `decode_file_uri`, `image_filters`, `response_to_outcome`. |
| `settings/src/app.rs` | Modify | `ColorSlot`; the Appearance and Behavior `Msg` arm groups of `update`; the `color_picker` field on `SettingsModel`. |
| `settings/src/main.rs` | Modify | `ICEDTEA_UI_THEME` and `ICEDTEA_SETTINGS_PAGE` knobs. |
| `settings/tests/support/mod.rs` | Create (shared with P1, P2-D2) | Spawn the settings binary under the harness, read its probe report, sample screencopy pixels, locate the client area. |
| `settings/tests/appearance.rs` | Create | The Appearance and Behavior rest-state gates and the two interaction gates. |
| `settings/tests/portal_worker.rs` | Create | The portal worker's no-bus degradation proof (owns its process; P2-D5). |
| `settings/tests/appearance_gtk.rs` | Delete | The populate-guard property is true by construction on an Elm loop (contract §2.2). |
| `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` | Modify (§6 only) | Append `P2-D1 … P2-D12`. |

---

## Task 1: Appearance page — wallpaper validation and spin-button clamping

**Files:**
- Modify: `settings/src/pages/appearance.rs`
- Test: `settings/src/pages/appearance.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from P1, contract §2.1):
  ```rust
  pub fn hex_to_rgba(s: &str) -> icedtea_ui::css::value::Rgba;
  pub fn rgba_to_hex(c: icedtea_ui::css::value::Rgba) -> String;
  pub fn hex_to_packed(s: &str) -> f64;
  pub fn packed_to_hex(packed: f64) -> String;
  ```
- Produces:
  ```rust
  pub const WALLPAPER_EXTENSIONS: [&str; 5];              // ["png", "jpg", "jpeg", "webp", "bmp"]
  pub fn validate_wallpaper(text: &str) -> Result<std::path::PathBuf, String>;
  pub fn spin_px(v: f64) -> i32;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/appearance.rs`'s existing `#[cfg(test)] mod tests` (create the module if P1's extraction left none):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;

    #[test]
    fn packed_round_trips_through_the_color_dialog_packing() {
        for hex in ["#1e1e2e", "#cdd6f4", "#89b4fa", "#000000", "#ffffff"] {
            assert_eq!(packed_to_hex(hex_to_packed(hex)), hex);
            assert_eq!(rgba_to_hex(ColorDialogC::unpack(hex_to_packed(hex))), hex);
        }
    }

    #[test]
    fn wallpaper_validation_rejects_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.png");
        let err = validate_wallpaper(&missing.display().to_string())
            .expect_err("a path that does not exist must not validate");
        assert!(
            err.contains("No such file"),
            "the message names the failure: {err:?}"
        );
    }

    #[test]
    fn wallpaper_validation_rejects_an_unknown_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wallpaper.txt");
        std::fs::write(&path, b"not an image").expect("write the file");
        let err = validate_wallpaper(&path.display().to_string())
            .expect_err("an existing non-image must not validate");
        assert!(
            err.contains("Unsupported image type"),
            "the message names the failure: {err:?}"
        );
    }

    #[test]
    fn wallpaper_validation_accepts_every_supported_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        for ext in WALLPAPER_EXTENSIONS {
            let path = dir.path().join(format!("wallpaper.{ext}"));
            std::fs::write(&path, b"pixels").expect("write the file");
            assert_eq!(
                validate_wallpaper(&path.display().to_string()).expect("validates"),
                path,
                "`.{ext}` is in WALLPAPER_EXTENSIONS"
            );
        }
    }

    #[test]
    fn wallpaper_validation_is_case_insensitive_about_the_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Wallpaper.PNG");
        std::fs::write(&path, b"pixels").expect("write the file");
        assert_eq!(
            validate_wallpaper(&path.display().to_string()).expect("validates"),
            path
        );
    }

    #[test]
    fn wallpaper_validation_rejects_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = validate_wallpaper(&dir.path().display().to_string())
            .expect_err("a directory is not a wallpaper");
        assert!(err.contains("Not a file"), "{err:?}");
    }

    #[test]
    fn spin_px_clamps_and_rounds_without_panicking() {
        assert_eq!(spin_px(0.0), 0);
        assert_eq!(spin_px(27.4), 27);
        assert_eq!(spin_px(27.6), 28);
        assert_eq!(spin_px(-5.0), 0, "below the range floor");
        assert_eq!(spin_px(9_000.0), 256, "above the range ceiling");
        assert_eq!(spin_px(f64::NAN), 0, "a non-finite value is not a panic");
        assert_eq!(spin_px(f64::INFINITY), 256);
    }
}
```

`tempfile` is already a `settings` dev-dependency (contract §2.1's `settings/Cargo.toml`), so no manifest change is needed.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::appearance -- --nocapture`
Expected: FAIL — `cannot find function 'validate_wallpaper' in this scope`, `cannot find function 'spin_px' in this scope`, `cannot find value 'WALLPAPER_EXTENSIONS' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `settings/src/pages/appearance.rs`, above the test module:

```rust
use std::path::PathBuf;

/// Image extensions the wallpaper field accepts, lower-case.
///
/// The compositor's wallpaper worker decodes with the same set; anything
/// outside it would be accepted here and silently ignored there, which is
/// what the inline error the `Entry` shows exists to prevent.
pub const WALLPAPER_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];

/// Validate a typed or portal-supplied wallpaper path.
///
/// `Ok(path)` is what goes into `working.appearance.wallpaper`; `Err(message)`
/// is what the page shows beside the field and is never written to the model.
/// The caller has already decided that an empty string means "no wallpaper",
/// so this function's caller never passes one.
///
/// Untrusted input: the string comes from a text field or a portal reply, so
/// every failure is a message and never a panic.
pub fn validate_wallpaper(text: &str) -> Result<PathBuf, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Enter a path to an image".to_string());
    }
    let path = PathBuf::from(trimmed);
    let meta = std::fs::metadata(&path).map_err(|_| format!("No such file: {trimmed}"))?;
    if !meta.is_file() {
        return Err(format!("Not a file: {trimmed}"));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !WALLPAPER_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "Unsupported image type: {}",
            if ext.is_empty() {
                "no extension".to_string()
            } else {
                format!(".{ext}")
            }
        ));
    }
    Ok(path)
}

/// The pixel value a `SpinButton` on this page reports, as the config stores
/// it.
///
/// The GTK page took `sb.value() as i32` over a `0.0..=256.0` adjustment; the
/// toolkit's `SpinButtonC` clamps to its own bounds too, but a hand-edited db
/// or a future range change must not be able to write a negative bar height,
/// and `as i32` on a NaN is 0 by saturation rather than by intent. Rounding
/// (not truncating) is what a 27.999 step lands on.
#[must_use]
pub fn spin_px(v: f64) -> i32 {
    if !v.is_finite() {
        return if v.is_sign_positive() && v.is_infinite() {
            SPIN_MAX_PX
        } else {
            0
        };
    }
    v.round().clamp(0.0, f64::from(SPIN_MAX_PX)) as i32
}

/// The upper bound every pixel spin button on this page shares, matching the
/// GTK `SpinButton::with_range(0.0, 256.0, 1.0)` adjustments.
pub const SPIN_MAX_PX: i32 = 256;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::appearance`
Expected: PASS — 7 tests in `pages::appearance::tests` (the two P1 hex tests plus the five added here), 0 failed.

- [ ] **Step 5: Mutation check**

Change `v.round().clamp(0.0, f64::from(SPIN_MAX_PX)) as i32` to `v as i32`.
Run: `cargo test -p icedtea-settings --lib pages::appearance::tests::spin_px_clamps_and_rounds_without_panicking`
Expected: FAIL — `assertion 'left == right' failed: left: 27, right: 28`.
Restore the line and re-run; expected PASS.

Then delete the `WALLPAPER_EXTENSIONS.contains(..)` guard.
Run: `cargo test -p icedtea-settings --lib pages::appearance::tests::wallpaper_validation_rejects_an_unknown_extension`
Expected: FAIL — `an existing non-image must not validate`.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/pages/appearance.rs
git commit -m "feat(settings): wallpaper path validation and spin-button clamping

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 2: Portal worker — the pure halves

**Files:**
- Create (or replace P1's stub body): `settings/src/ipc/portal.rs`
- Test: `settings/src/ipc/portal.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from earlier M5 tasks; `zbus::zvariant` types only.
- Produces:
  ```rust
  pub const PORTAL_BUS: &str;          // "org.freedesktop.portal.Desktop"
  pub const PORTAL_PATH: &str;         // "/org/freedesktop/portal/desktop"
  pub const FILE_CHOOSER_IFACE: &str;  // "org.freedesktop.portal.FileChooser"
  pub const REQUEST_IFACE: &str;       // "org.freedesktop.portal.Request"

  pub fn request_object_path(unique_name: &str, token: &str) -> String;
  pub fn decode_file_uri(uri: &str) -> Option<std::path::PathBuf>;
  pub fn image_filters() -> Vec<(String, Vec<(u32, String)>)>;
  pub fn response_to_outcome(
      response: u32,
      results: &std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
  ) -> Result<std::path::PathBuf, String>;
  ```

- [ ] **Step 1: Write the failing tests**

Create `settings/src/ipc/portal.rs` with only the test module and the `use` lines it needs (the implementation arrives in Step 3; if P1 left a stub body in this file, keep the file and append this module):

```rust
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use zbus::zvariant::{OwnedValue, Value};

    use super::*;

    fn results(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
        pairs
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    OwnedValue::try_from(v).expect("value converts"),
                )
            })
            .collect()
    }

    #[test]
    fn portal_uri_decoding_handles_percent_escapes() {
        assert_eq!(
            decode_file_uri("file:///home/me/My%20Pictures/wall%2Bpaper.png"),
            Some(PathBuf::from("/home/me/My Pictures/wall+paper.png"))
        );
        assert_eq!(
            decode_file_uri("file:///tmp/plain.png"),
            Some(PathBuf::from("/tmp/plain.png"))
        );
        assert_eq!(
            decode_file_uri("file://localhost/tmp/plain.png"),
            Some(PathBuf::from("/tmp/plain.png")),
            "an explicit localhost authority is the same local file"
        );
    }

    #[test]
    fn portal_uri_decoding_rejects_what_is_not_a_local_file() {
        assert_eq!(decode_file_uri("https://example.invalid/a.png"), None);
        assert_eq!(decode_file_uri("file://remote-host/tmp/a.png"), None);
        assert_eq!(decode_file_uri(""), None);
        assert_eq!(decode_file_uri("file:///tmp/%zz.png"), None, "bad escape");
        assert_eq!(
            decode_file_uri("file:///tmp/%00.png"),
            None,
            "an interior NUL never becomes a path"
        );
    }

    #[test]
    fn a_cancelled_response_is_a_failure_message_not_a_path() {
        let err = response_to_outcome(1, &results(vec![])).expect_err("cancelled");
        assert!(err.contains("cancelled"), "{err:?}");
        let err = response_to_outcome(2, &results(vec![])).expect_err("ended");
        assert!(!err.is_empty());
    }

    #[test]
    fn an_empty_or_missing_uri_list_is_a_failure_message() {
        let err = response_to_outcome(0, &results(vec![])).expect_err("no uris key");
        assert!(err.contains("no file"), "{err:?}");
        let empty: Vec<String> = Vec::new();
        let err = response_to_outcome(0, &results(vec![("uris", Value::from(empty))]))
            .expect_err("empty uris");
        assert!(err.contains("no file"), "{err:?}");
    }

    #[test]
    fn the_first_local_uri_wins() {
        let uris = vec!["file:///tmp/one.png".to_string()];
        assert_eq!(
            response_to_outcome(0, &results(vec![("uris", Value::from(uris))])).expect("a path"),
            PathBuf::from("/tmp/one.png")
        );
    }

    #[test]
    fn a_request_path_is_the_documented_sender_derived_one() {
        assert_eq!(
            request_object_path(":1.42", "icedtea_0"),
            "/org/freedesktop/portal/desktop/request/1_42/icedtea_0"
        );
        assert_eq!(
            request_object_path("1.42", "t"),
            "/org/freedesktop/portal/desktop/request/1_42/t",
            "a name that already lost its colon is handled the same"
        );
    }

    #[test]
    fn the_image_filter_covers_every_accepted_extension() {
        let filters = image_filters();
        let patterns: Vec<String> = filters
            .iter()
            .flat_map(|(_, globs)| globs.iter().map(|(_, g)| g.clone()))
            .collect();
        for ext in crate::pages::appearance::WALLPAPER_EXTENSIONS {
            assert!(
                patterns.iter().any(|p| p == &format!("*.{ext}")),
                "the portal filter offers .{ext}"
            );
        }
        assert!(
            filters.iter().all(|(_, globs)| globs.iter().all(|(k, _)| *k == 0)),
            "0 is the glob-pattern filter kind; 1 would be a MIME type"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib ipc::portal`
Expected: FAIL to compile — `cannot find function 'decode_file_uri' in this scope` and five more of the same shape.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `settings/src/ipc/portal.rs`:

```rust
//! The `org.freedesktop.portal.FileChooser` wallpaper picker (spec D5).
//!
//! One worker thread, blocking zbus, answering on P0's inbox. The `Entry`
//! beside the Browse button is always live and always authoritative
//! (contract §2.6) — this worker is a convenience, and every one of its
//! failure modes is a single `Msg::WallpaperPickerFailed` the status line
//! shows, never a panic and never a hang.

use std::collections::HashMap;
use std::path::PathBuf;

/// The portal's well-known bus name.
pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
/// The portal's object path.
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// The file-chooser interface.
pub const FILE_CHOOSER_IFACE: &str = "org.freedesktop.portal.FileChooser";
/// The interface the asynchronous answer arrives on.
pub const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// The object path the portal will emit `Response` on, derived the documented
/// way from our unique bus name and the `handle_token` we send.
///
/// Deriving it lets the worker subscribe *before* it calls `OpenFile`, which
/// is the only way to be sure a fast answer is not missed.
#[must_use]
pub fn request_object_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// Decode a `file://` URI into a local path.
///
/// `None` for anything that is not a local file: another scheme, a non-empty
/// authority that is not `localhost`, a malformed percent escape, a byte
/// sequence that is not UTF-8, or an interior NUL. The URI comes from another
/// process, so none of those is a panic.
#[must_use]
pub fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path_part = match rest.find('/') {
        Some(0) => rest,
        Some(slash) if &rest[..slash] == "localhost" => &rest[slash..],
        _ => return None,
    };
    let bytes = percent_decode(path_part)?;
    if bytes.contains(&0) {
        return None;
    }
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

/// `%XX` decoding. `None` on a truncated or non-hex escape.
fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    Some(out)
}

/// The `filters` option: one group, one glob per accepted extension.
///
/// The `a(sa(us))` signature is `(name, [(kind, pattern)])`; kind `0` is a
/// shell glob and kind `1` is a MIME type. Globs, not MIME types, because the
/// same extension list is what [`crate::pages::appearance::validate_wallpaper`]
/// enforces on the way back in — offering a MIME filter the validator does not
/// share would let the portal return a file the field then rejects.
#[must_use]
pub fn image_filters() -> Vec<(String, Vec<(u32, String)>)> {
    let globs = crate::pages::appearance::WALLPAPER_EXTENSIONS
        .iter()
        .map(|ext| (0u32, format!("*.{ext}")))
        .collect();
    vec![("Images".to_string(), globs)]
}

/// Turn one `org.freedesktop.portal.Request.Response` body into an outcome.
///
/// `response` is `0` for success, `1` for "the user cancelled" and `2` for
/// "ended some other way" (which is also what our own `Request.Close` on a
/// timeout produces).
pub fn response_to_outcome(
    response: u32,
    results: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Result<PathBuf, String> {
    match response {
        0 => {}
        1 => return Err("Wallpaper selection cancelled".to_string()),
        other => return Err(format!("The file portal ended the request (code {other})")),
    }
    let uris = results
        .get("uris")
        .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
        .unwrap_or_default();
    uris.iter()
        .find_map(|uri| decode_file_uri(uri))
        .ok_or_else(|| "The file portal returned no file this app can open".to_string())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib ipc::portal`
Expected: PASS — 6 tests, 0 failed.

- [ ] **Step 5: Mutation check**

In `decode_file_uri`, replace `let bytes = percent_decode(path_part)?;` with `let bytes = path_part.as_bytes().to_vec();`.
Run: `cargo test -p icedtea-settings --lib ipc::portal::tests::portal_uri_decoding_handles_percent_escapes`
Expected: FAIL — the decoded path still contains `%20`.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/ipc/portal.rs
git commit -m "feat(settings): portal request-path, URI and response decoding

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 3: Portal worker — the thread

**Files:**
- Modify: `settings/src/ipc/portal.rs`
- Create: `settings/tests/portal_worker.rs`

**Interfaces:**
- Consumes:
  - Task 2's `request_object_path`, `decode_file_uri`, `image_filters`, `response_to_outcome`, and the four `*_IFACE`/`*_BUS`/`*_PATH` constants.
  - From P0 (M5-D2): `icedtea_ui::view::{Inbox, InboxSender}`; `InboxSender::send(&self, msg) -> Result<(), SendError<Msg>>`, `Inbox::new() -> std::io::Result<(Inbox<Msg>, InboxSender<Msg>)>`.
  - From P1 (contract §2.2): `crate::app::Msg::{WallpaperChosen(PathBuf), WallpaperPickerFailed(String)}`.
- Produces:
  ```rust
  pub enum PortalRequest { OpenFile { current: Option<std::path::PathBuf> } }
  pub fn spawn(
      rx: crossbeam_channel::Receiver<PortalRequest>,
      tx: icedtea_ui::view::InboxSender<crate::app::Msg>,
  ) -> std::thread::JoinHandle<()>;
  pub const PORTAL_TIMEOUT: std::time::Duration;   // 300 s
  ```

- [ ] **Step 1: Write the failing test**

Create `settings/tests/portal_worker.rs`:

```rust
//! The portal worker's degradation proof.
//!
//! Owns its own test binary because it edits `DBUS_SESSION_BUS_ADDRESS`,
//! which is process-global: a second test running concurrently in the same
//! binary would see the doctored value. Same reasoning the deleted
//! `appearance_gtk.rs` gave for GTK's one-init-per-process rule.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use icedtea_settings::app::Msg;
use icedtea_settings::ipc::portal::{self, PortalRequest};
use icedtea_ui::view::Inbox;

/// Generous: this asserts "the worker answers rather than hanging", not how
/// fast a failed bus connection is refused.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

fn wait_for_answer(inbox: &Inbox<Msg>) -> Msg {
    let started = Instant::now();
    while started.elapsed() < ANSWER_TIMEOUT {
        if let Some(msg) = inbox.try_recv() {
            return msg;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the portal worker never answered within {ANSWER_TIMEOUT:?}");
}

#[test]
fn a_missing_session_bus_answers_with_a_picker_failure() {
    // SAFETY: one test per binary, set before the worker thread starts, and
    // never read by anything else in this process.
    unsafe {
        std::env::set_var(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/icedtea-no-such-bus",
        );
    }

    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let (req_tx, req_rx) = crossbeam_channel::unbounded();
    let worker = portal::spawn(req_rx, tx);

    req_tx
        .send(PortalRequest::OpenFile { current: None })
        .expect("queue the request");

    match wait_for_answer(&inbox) {
        Msg::WallpaperPickerFailed(reason) => {
            assert!(
                !reason.is_empty(),
                "the failure carries a message for the status line"
            );
        }
        other => panic!("expected a picker failure, got {other:?}"),
    }

    drop(req_tx);
    worker.join().expect("the worker exits when its queue closes");
}

#[test]
fn a_request_queued_behind_another_supersedes_it() {
    // Two requests queued before the worker can pick either up: the worker
    // must answer once, for the *last* one, not twice.
    unsafe {
        std::env::set_var(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/icedtea-no-such-bus",
        );
    }
    let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
    let (req_tx, req_rx) = crossbeam_channel::unbounded();
    req_tx
        .send(PortalRequest::OpenFile { current: None })
        .expect("first");
    req_tx
        .send(PortalRequest::OpenFile {
            current: Some(PathBuf::from("/tmp")),
        })
        .expect("second");
    let worker = portal::spawn(req_rx, tx);

    let first = wait_for_answer(&inbox);
    assert!(matches!(first, Msg::WallpaperPickerFailed(_)));

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        inbox.try_recv().is_none(),
        "the superseded request produced no second answer"
    );

    drop(req_tx);
    worker.join().expect("the worker exits");
}
```

`Inbox::try_recv` is P0's receiving half used directly by a test rather than by `App::run`; it is part of M5-D2's `Inbox<Msg>` surface (the drain `App::run` performs). If P0 shipped `Inbox` without a public `try_recv`, add it in this task's Step 3 as a one-line `pub fn try_recv(&self) -> Option<Msg>` delegating to the channel — record it as a further P0 amendment note in Task 11's §6 append.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-settings --test portal_worker`
Expected: FAIL to compile — `cannot find function 'spawn' in module 'portal'`, `cannot find type 'PortalRequest'`.

- [ ] **Step 3: Write the implementation**

Append to `settings/src/ipc/portal.rs` (above its test module):

```rust
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use icedtea_ui::view::InboxSender;

use crate::app::Msg;

/// What the loop asks the portal worker for.
pub enum PortalRequest {
    /// Open the file chooser, starting at `current`'s directory if it has one.
    OpenFile { current: Option<PathBuf> },
}

/// How long the worker waits for a `Response` before closing the request.
///
/// A file dialog is user-driven, so this is a ceiling on a wedged portal, not
/// a UX budget: on expiry the worker calls `Request.Close`, which makes the
/// portal answer with a non-zero response, and the user sees a status line
/// instead of a Browse button that never comes back.
pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// One worker thread serving `rx` until every sender is dropped.
///
/// Per request: run the whole `OpenFile` exchange, then send exactly one of
/// [`Msg::WallpaperChosen`] or [`Msg::WallpaperPickerFailed`]. A send error
/// means the app exited, and the thread returns.
///
/// **Superseding.** A request found waiting behind another is the one the
/// user meant: the worker drains everything already queued and serves only
/// the last, answering once.
pub fn spawn(
    rx: crossbeam_channel::Receiver<PortalRequest>,
    tx: InboxSender<Msg>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut request = first;
            while let Ok(newer) = rx.try_recv() {
                request = newer;
            }
            let PortalRequest::OpenFile { current } = request;
            let msg = match open_file(current.as_deref()) {
                Ok(path) => Msg::WallpaperChosen(path),
                Err(reason) => Msg::WallpaperPickerFailed(reason),
            };
            if tx.send(msg).is_err() {
                return;
            }
        }
    })
}

/// The whole exchange, blocking, on the worker thread.
///
/// Subscribe first (the request path is derived, not read off the reply, so a
/// fast answer cannot be missed), then call `OpenFile`, then wait for the
/// `Response` signal on that path.
fn open_file(current: Option<&std::path::Path>) -> Result<PathBuf, String> {
    let conn = Connection::session()
        .map_err(|err| format!("No session bus for the file portal: {err}"))?;
    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_string())
        .ok_or_else(|| "The session bus gave this app no unique name".to_string())?;
    let token = format!("icedtea_{}", std::process::id());
    let path = request_object_path(&unique, &token);

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(REQUEST_IFACE)
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .member("Response")
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .path(path.as_str())
        .map_err(|err| format!("Bad portal request path: {err}"))?
        .build();
    let mut signals = MessageIterator::for_match_rule(rule, &conn, Some(4))
        .map_err(|err| format!("Cannot listen for the portal's answer: {err}"))?;

    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    options.insert("multiple", Value::from(false));
    options.insert(
        "filters",
        Value::new(image_filters()).try_clone().map_err(|err| {
            format!("Cannot build the portal's image filter: {err}")
        })?,
    );
    if let Some(folder) = current.and_then(std::path::Path::parent) {
        let mut bytes = folder.as_os_str().as_encoded_bytes().to_vec();
        bytes.push(0);
        options.insert("current_folder", Value::from(bytes));
    }

    let reply = conn
        .call_method(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            Some(FILE_CHOOSER_IFACE),
            "OpenFile",
            &("", "Choose wallpaper", &options),
        )
        .map_err(|err| format!("The file portal is unavailable: {err}"))?;
    let handle: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|err| format!("The file portal answered with something else: {err}"))?;
    if handle.as_str() != path {
        tracing::warn!(
            expected = %path,
            got = %handle.as_str(),
            "the portal ignored handle_token; watching its own path instead"
        );
        return await_response_on(&conn, handle.as_str());
    }

    await_response(&mut signals, &conn, &path)
}

/// Wait on an already-open subscription.
fn await_response(
    signals: &mut MessageIterator,
    conn: &Connection,
    path: &str,
) -> Result<PathBuf, String> {
    let (done_tx, done_rx) = crossbeam_channel::bounded::<Result<PathBuf, String>>(1);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for message in signals.by_ref() {
                let Ok(message) = message else { continue };
                let body = message.body();
                let outcome = match body.deserialize::<(u32, HashMap<String, OwnedValue>)>() {
                    Ok((response, results)) => response_to_outcome(response, &results),
                    Err(err) => Err(format!("Unreadable portal answer: {err}")),
                };
                let _ = done_tx.send(outcome);
                return;
            }
            let _ = done_tx.send(Err("The file portal closed its connection".to_string()));
        });
        match done_rx.recv_timeout(PORTAL_TIMEOUT) {
            Ok(outcome) => outcome,
            Err(_) => {
                // Close the request so the portal answers and the reader
                // thread this scope is waiting on can finish.
                let _ = conn.call_method(
                    Some(PORTAL_BUS),
                    path,
                    Some(REQUEST_IFACE),
                    "Close",
                    &(),
                );
                done_rx
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap_or_else(|_| Err("The file portal did not answer".to_string()))
            }
        }
    })
}

/// The same wait, but subscribing after the fact — only reached when a portal
/// ignores `handle_token` and hands back a path of its own choosing.
fn await_response_on(conn: &Connection, path: &str) -> Result<PathBuf, String> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(REQUEST_IFACE)
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .member("Response")
        .map_err(|err| format!("Bad portal match rule: {err}"))?
        .path(path)
        .map_err(|err| format!("Bad portal request path: {err}"))?
        .build();
    let mut signals = MessageIterator::for_match_rule(rule, conn, Some(4))
        .map_err(|err| format!("Cannot listen for the portal's answer: {err}"))?;
    await_response(&mut signals, conn, path)
}
```

Also add, in `settings/src/ipc/mod.rs`, nothing: `WorkerHandles::choose_wallpaper` and `ipc::spawn` are P1's and already call `portal::spawn(portal_rx, tx.clone())` with these exact types.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p icedtea-settings --test portal_worker`
Expected: PASS — 2 tests, 0 failed. (The tests run in the same binary but each sets the same doctored bus address, so their order does not matter.)

- [ ] **Step 5: Mutation check**

In `spawn`, delete the `while let Ok(newer) = rx.try_recv() { request = newer; }` drain.
Run: `cargo test -p icedtea-settings --test portal_worker::a_request_queued_behind_another_supersedes_it`
Expected: FAIL — `the superseded request produced no second answer`.
Restore and re-run; expected PASS.

Then change `Connection::session().map_err(...)` to `Connection::session().expect("bus")`.
Run: `cargo test -p icedtea-settings --test portal_worker`
Expected: FAIL — the worker thread panics and `a_missing_session_bus_answers_with_a_picker_failure` times out instead of receiving a message. This is the "never panics on untrusted input" rule under test.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/ipc/portal.rs settings/tests/portal_worker.rs
git commit -m "feat(settings): FileChooser.OpenFile portal worker with bounded wait

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 4: `update` — the Appearance arms that are not the wallpaper

**Files:**
- Modify: `settings/src/app.rs`
- Test: `settings/src/app.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes:
  - From P1 (contract §2.2/§2.3): `pub struct SettingsModel { pub model, pub db_path, pub page, pub status, pub capturing, pub conflicts, pub wallpaper_text, pub wallpaper_error, pub portal_available, pub displays, pub displays_status, pub displays_in_flight, pub outputs_available, pub workers }`; `pub enum Msg` with the Appearance group; `pub fn update(m: &mut SettingsModel, msg: Msg) -> Cmd<Msg>`; `pub fn view(m: &SettingsModel) -> View<Msg>`; `crate::ipc::{WorkerHandles, spawn}`.
  - From Task 1: `crate::pages::appearance::spin_px`.
  - From P1 (§2.1): `crate::pages::appearance::packed_to_hex`.
  - `icedtea_config::defaults::default_config`, `crate::model::{Model, BAR_POSITIONS}`.
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum ColorSlot { Background, Foreground, Accent }
  // added to SettingsModel:
  pub color_picker: Option<ColorSlot>,
  // added to Msg:
  ColorPickerOpened(ColorSlot),
  ColorPickerClosed,
  ```
  and a test-only constructor other tasks reuse:
  ```rust
  #[cfg(test)] pub(crate) fn test_model() -> (SettingsModel, icedtea_ui::view::Inbox<Msg>);
  ```

- [ ] **Step 1: Write the failing tests**

Add to `settings/src/app.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A model with live workers and no window. The returned `Inbox` must be
    /// held: dropping it makes every worker's next `send` fail and the worker
    /// threads exit, which is correct behaviour but confusing mid-test.
    pub(crate) fn test_model() -> (SettingsModel, icedtea_ui::view::Inbox<Msg>) {
        let (inbox, tx) = icedtea_ui::view::Inbox::<Msg>::new().expect("inbox");
        let workers = crate::ipc::spawn(tx);
        let cfg = icedtea_config::default_config();
        let model = SettingsModel {
            model: crate::model::Model {
                working: cfg.clone(),
                saved: cfg,
            },
            db_path: std::path::PathBuf::from("/nonexistent/icedtea-test.redb"),
            page: crate::pages::PageId::Appearance,
            status: String::new(),
            capturing: None,
            conflicts: Vec::new(),
            wallpaper_text: String::new(),
            wallpaper_error: None,
            portal_available: true,
            color_picker: None,
            displays: crate::pages::displays::state::DisplaysState::new(),
            displays_status: String::new(),
            displays_in_flight: false,
            outputs_available: false,
            workers,
        };
        (model, inbox)
    }

    #[test]
    fn a_bar_position_pick_writes_the_domain_value_not_the_index() {
        let (mut m, _inbox) = test_model();
        update(&mut m, Msg::BarPositionSelected(1));
        assert_eq!(m.model.working.appearance.bar_position, "bottom");
        update(&mut m, Msg::BarPositionSelected(0));
        assert_eq!(m.model.working.appearance.bar_position, "top");
    }

    #[test]
    fn an_out_of_domain_bar_position_index_is_ignored() {
        let (mut m, _inbox) = test_model();
        let before = m.model.working.appearance.bar_position.clone();
        update(&mut m, Msg::BarPositionSelected(99));
        assert_eq!(
            m.model.working.appearance.bar_position, before,
            "an index BAR_POSITIONS does not have leaves the model alone"
        );
    }

    #[test]
    fn the_pixel_spin_buttons_write_clamped_integers() {
        let (mut m, _inbox) = test_model();
        update(&mut m, Msg::BarHeightChanged(31.6));
        update(&mut m, Msg::CornerRadiusChanged(-4.0));
        assert_eq!(m.model.working.appearance.bar_height, 32);
        assert_eq!(m.model.working.appearance.corner_radius, 0);
        assert!(m.model.is_dirty(), "an edit makes the model dirty");
    }

    #[test]
    fn a_colour_pick_writes_hex_and_closes_the_picker() {
        let (mut m, _inbox) = test_model();
        update(&mut m, Msg::ColorPickerOpened(ColorSlot::Accent));
        assert_eq!(m.color_picker, Some(ColorSlot::Accent));
        let packed = crate::pages::appearance::hex_to_packed("#89b4fa");
        update(&mut m, Msg::AccentPicked(packed));
        assert_eq!(m.model.working.appearance.palette.accent, "#89b4fa");
        assert_eq!(m.color_picker, None, "picking closes the picker");
    }

    #[test]
    fn the_three_colour_slots_are_independent() {
        let (mut m, _inbox) = test_model();
        update(
            &mut m,
            Msg::BackgroundPicked(crate::pages::appearance::hex_to_packed("#1e1e2e")),
        );
        update(
            &mut m,
            Msg::ForegroundPicked(crate::pages::appearance::hex_to_packed("#cdd6f4")),
        );
        let palette = &m.model.working.appearance.palette;
        assert_eq!(palette.background, "#1e1e2e");
        assert_eq!(palette.foreground, "#cdd6f4");
        assert_eq!(
            palette.accent,
            icedtea_config::default_config().appearance.palette.accent,
            "the untouched slot is untouched"
        );
    }

    #[test]
    fn closing_the_picker_leaves_the_model_alone() {
        let (mut m, _inbox) = test_model();
        update(&mut m, Msg::ColorPickerOpened(ColorSlot::Background));
        update(&mut m, Msg::ColorPickerClosed);
        assert_eq!(m.color_picker, None);
        assert!(!m.model.is_dirty(), "opening and closing edits nothing");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: FAIL to compile — `no variant named 'ColorPickerOpened' found for enum 'Msg'`, `struct 'SettingsModel' has no field named 'color_picker'`.

- [ ] **Step 3: Write the implementation**

In `settings/src/app.rs`:

```rust
/// Which palette entry the colour picker is editing.
///
/// The picker is a panel the Appearance page renders under the palette rows
/// (contract deviation P2-D11); this is the only state it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSlot {
    Background,
    Foreground,
    Accent,
}
```

Add to `SettingsModel`, immediately after `portal_available`:

```rust
    /// The palette entry whose picker panel is open, if any.
    pub color_picker: Option<ColorSlot>,
```

Add to `Msg`, in the Appearance group:

```rust
    ColorPickerOpened(ColorSlot),
    ColorPickerClosed,
```

Replace the Appearance arm group of `update` (P1 shipped these as `Cmd::None` no-ops) with:

```rust
        Msg::BarPositionSelected(index) => {
            if let Some(position) = crate::model::BAR_POSITIONS.get(index) {
                m.model.working.appearance.bar_position = (*position).to_string();
            }
            Cmd::None
        }
        Msg::BarHeightChanged(v) => {
            m.model.working.appearance.bar_height = crate::pages::appearance::spin_px(v);
            Cmd::None
        }
        Msg::CornerRadiusChanged(v) => {
            m.model.working.appearance.corner_radius = crate::pages::appearance::spin_px(v);
            Cmd::None
        }
        Msg::ColorPickerOpened(slot) => {
            m.color_picker = Some(slot);
            Cmd::None
        }
        Msg::ColorPickerClosed => {
            m.color_picker = None;
            Cmd::None
        }
        Msg::BackgroundPicked(packed) => {
            m.model.working.appearance.palette.background =
                crate::pages::appearance::packed_to_hex(packed);
            m.color_picker = None;
            Cmd::None
        }
        Msg::ForegroundPicked(packed) => {
            m.model.working.appearance.palette.foreground =
                crate::pages::appearance::packed_to_hex(packed);
            m.color_picker = None;
            Cmd::None
        }
        Msg::AccentPicked(packed) => {
            m.model.working.appearance.palette.accent =
                crate::pages::appearance::packed_to_hex(packed);
            m.color_picker = None;
            Cmd::None
        }
```

Finally, wherever P1 builds the initial `SettingsModel` (its `SettingsModel::new`), add `color_picker: None,`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: PASS — 6 tests, 0 failed.

- [ ] **Step 5: Mutation check**

Change `if let Some(position) = crate::model::BAR_POSITIONS.get(index)` to `let position = crate::model::BAR_POSITIONS[index.min(1)];` and assign unconditionally.
Run: `cargo test -p icedtea-settings --lib app::tests::an_out_of_domain_bar_position_index_is_ignored`
Expected: FAIL — the model's `bar_position` became `"bottom"`.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/app.rs
git commit -m "feat(settings): Appearance update arms and the colour-picker slot

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 5: `update` — the wallpaper arms

**Files:**
- Modify: `settings/src/app.rs`
- Test: `settings/src/app.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 4's `test_model`; Task 1's `crate::pages::appearance::validate_wallpaper`; P1's `WorkerHandles::choose_wallpaper(&self, current: Option<std::path::PathBuf>)`; `icedtea_ui::view::Cmd`.
- Produces: the fully-implemented `Msg::{WallpaperEdited, WallpaperBrowse, WallpaperChosen, WallpaperPickerFailed, WallpaperCleared}` arms. No new public names.

- [ ] **Step 1: Write the failing tests**

Add to `settings/src/app.rs`'s `mod tests`:

```rust
    fn a_real_image(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"pixels").expect("write the image");
        path
    }

    #[test]
    fn a_valid_typed_path_reaches_the_model_and_clears_the_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = a_real_image(dir.path(), "wall.png");
        let (mut m, _inbox) = test_model();
        m.wallpaper_error = Some("stale".to_string());

        update(&mut m, Msg::WallpaperEdited(image.display().to_string()));

        assert_eq!(m.wallpaper_text, image.display().to_string());
        assert_eq!(m.wallpaper_error, None);
        assert_eq!(
            m.model.working.appearance.wallpaper,
            Some(image.display().to_string())
        );
    }

    #[test]
    fn an_invalid_typed_path_shows_an_error_and_never_touches_the_model() {
        let (mut m, _inbox) = test_model();
        let before = m.model.working.appearance.wallpaper.clone();

        update(
            &mut m,
            Msg::WallpaperEdited("/nonexistent/icedtea/wall.png".to_string()),
        );

        assert_eq!(m.wallpaper_text, "/nonexistent/icedtea/wall.png");
        assert!(m.wallpaper_error.is_some(), "the field shows why");
        assert_eq!(
            m.model.working.appearance.wallpaper, before,
            "an invalid path is never written to the working copy"
        );
    }

    #[test]
    fn clearing_the_field_clears_the_wallpaper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = a_real_image(dir.path(), "wall.png");
        let (mut m, _inbox) = test_model();
        update(&mut m, Msg::WallpaperEdited(image.display().to_string()));

        update(&mut m, Msg::WallpaperEdited("   ".to_string()));
        assert_eq!(m.model.working.appearance.wallpaper, None);
        assert_eq!(m.wallpaper_error, None, "empty is not an error");

        update(&mut m, Msg::WallpaperEdited(image.display().to_string()));
        update(&mut m, Msg::WallpaperCleared);
        assert!(m.wallpaper_text.is_empty());
        assert_eq!(m.model.working.appearance.wallpaper, None);
        assert_eq!(m.wallpaper_error, None);
    }

    #[test]
    fn a_portal_answer_goes_through_the_same_validation() {
        let (mut m, _inbox) = test_model();
        update(
            &mut m,
            Msg::WallpaperChosen(std::path::PathBuf::from("/nonexistent/portal/wall.png")),
        );
        assert!(
            m.wallpaper_error.is_some(),
            "the portal is not trusted more than the keyboard"
        );
        assert_eq!(m.model.working.appearance.wallpaper, None);
    }

    #[test]
    fn a_failed_picker_disables_browse_without_touching_the_model() {
        let (mut m, _inbox) = test_model();
        let before = m.model.working.clone();

        update(
            &mut m,
            Msg::WallpaperPickerFailed("No file portal available".to_string()),
        );

        assert!(!m.portal_available, "Browse is greyed for the session");
        assert_eq!(m.status, "No file portal available");
        assert_eq!(m.model.working, before, "the working copy is untouched");
        assert!(!m.model.is_dirty());
    }

    #[test]
    fn browsing_without_a_portal_is_a_status_line_not_a_task() {
        let (mut m, _inbox) = test_model();
        m.portal_available = false;
        let cmd = update(&mut m, Msg::WallpaperBrowse);
        assert!(
            matches!(cmd, Cmd::None),
            "no Cmd::Task is issued once the portal is known missing"
        );
        assert!(!m.status.is_empty());
    }

    #[test]
    fn browsing_with_a_portal_issues_a_task() {
        let (mut m, _inbox) = test_model();
        let cmd = update(&mut m, Msg::WallpaperBrowse);
        assert!(
            matches!(cmd, Cmd::Task(_)),
            "the outbound call runs on the worker, never inside update"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: FAIL — `a_valid_typed_path_reaches_the_model_and_clears_the_error` fails with `assertion 'left == right' failed: left: None, right: Some("…/wall.png")` (P1's arms are no-ops), and `browsing_with_a_portal_issues_a_task` fails with `the outbound call runs on the worker, never inside update`.

- [ ] **Step 3: Write the implementation**

In `settings/src/app.rs`, add above `update`:

```rust
/// Fold a candidate wallpaper path into the model.
///
/// One code path for the `Entry` and for the portal's answer: the portal is
/// another process and its reply is no more trusted than a typed string.
fn set_wallpaper(m: &mut SettingsModel, text: String) {
    m.wallpaper_text = text;
    if m.wallpaper_text.trim().is_empty() {
        m.wallpaper_error = None;
        m.model.working.appearance.wallpaper = None;
        return;
    }
    match crate::pages::appearance::validate_wallpaper(&m.wallpaper_text) {
        Ok(path) => {
            m.wallpaper_error = None;
            m.model.working.appearance.wallpaper = Some(path.display().to_string());
        }
        Err(reason) => m.wallpaper_error = Some(reason),
    }
}
```

and replace the wallpaper arms:

```rust
        Msg::WallpaperEdited(text) => {
            set_wallpaper(m, text);
            Cmd::None
        }
        Msg::WallpaperChosen(path) => {
            set_wallpaper(m, path.display().to_string());
            Cmd::None
        }
        Msg::WallpaperCleared => {
            set_wallpaper(m, String::new());
            Cmd::None
        }
        Msg::WallpaperBrowse => {
            if !m.portal_available {
                m.status = "No file portal available; type a path instead".to_string();
                return Cmd::None;
            }
            let handles = m.workers.clone();
            let current = m
                .model
                .working
                .appearance
                .wallpaper
                .as_ref()
                .map(std::path::PathBuf::from);
            Cmd::Task(std::rc::Rc::new(move || {
                handles.choose_wallpaper(current.clone());
            }))
        }
        Msg::WallpaperPickerFailed(reason) => {
            m.status = reason;
            m.portal_available = false;
            Cmd::None
        }
```

`update`'s body already returns `Cmd<Msg>` from a `match`; the `return Cmd::None` above requires `update` to be an ordinary `fn` with the match as its tail expression, which is what contract §2.2 specifies.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: PASS — 13 tests, 0 failed.

- [ ] **Step 5: Mutation check**

In `set_wallpaper`'s `Err(reason)` arm, add `m.model.working.appearance.wallpaper = Some(m.wallpaper_text.clone());` before the assignment to `wallpaper_error`.
Run: `cargo test -p icedtea-settings --lib app::tests::an_invalid_typed_path_shows_an_error_and_never_touches_the_model`
Expected: FAIL — `an invalid path is never written to the working copy`.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/app.rs
git commit -m "feat(settings): wallpaper update arms with one validation path

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 6: `update` — the Behavior arms

**Files:**
- Modify: `settings/src/app.rs`
- Test: `settings/src/app.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 4's `test_model`; Task 1's `spin_px`; `Msg::{RaiseOnFocusToggled, HideBarOnFullscreenToggled, SnapEnabledToggled, SnapGapChanged}`.
- Produces: the implemented Behavior arm group. No new public names.

- [ ] **Step 1: Write the failing tests**

Add to `settings/src/app.rs`'s `mod tests`:

```rust
    #[test]
    fn each_behavior_switch_writes_its_own_field() {
        let (mut m, _inbox) = test_model();
        let defaults = icedtea_config::default_config().behavior;

        update(&mut m, Msg::RaiseOnFocusToggled(!defaults.raise_on_focus));
        assert_eq!(
            m.model.working.behavior.raise_on_focus,
            !defaults.raise_on_focus
        );
        assert_eq!(
            m.model.working.behavior.hide_bar_on_fullscreen, defaults.hide_bar_on_fullscreen,
            "the other two switches are untouched"
        );
        assert_eq!(m.model.working.behavior.snap_enabled, defaults.snap_enabled);

        update(
            &mut m,
            Msg::HideBarOnFullscreenToggled(!defaults.hide_bar_on_fullscreen),
        );
        update(&mut m, Msg::SnapEnabledToggled(!defaults.snap_enabled));
        assert_eq!(
            m.model.working.behavior.hide_bar_on_fullscreen,
            !defaults.hide_bar_on_fullscreen
        );
        assert_eq!(m.model.working.behavior.snap_enabled, !defaults.snap_enabled);
    }

    #[test]
    fn toggling_a_switch_makes_the_model_dirty_and_toggling_back_makes_it_clean() {
        let (mut m, _inbox) = test_model();
        let before = m.model.working.behavior.raise_on_focus;
        update(&mut m, Msg::RaiseOnFocusToggled(!before));
        assert!(m.model.is_dirty());
        update(&mut m, Msg::RaiseOnFocusToggled(before));
        assert!(
            !m.model.is_dirty(),
            "dirty is computed from working != saved, never a latched flag"
        );
    }

    #[test]
    fn the_snap_gap_spin_writes_appearance_snap_gap_clamped() {
        let (mut m, _inbox) = test_model();
        update(&mut m, Msg::SnapGapChanged(11.5));
        assert_eq!(
            m.model.working.appearance.snap_gap, 12,
            "snap_gap lives on Appearance in the config even though the control is on Behavior"
        );
        update(&mut m, Msg::SnapGapChanged(-1.0));
        assert_eq!(m.model.working.appearance.snap_gap, 0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: FAIL — `each_behavior_switch_writes_its_own_field` fails on the first assertion (P1's arms are no-ops).

- [ ] **Step 3: Write the implementation**

Replace the Behavior arm group in `settings/src/app.rs`:

```rust
        Msg::RaiseOnFocusToggled(on) => {
            m.model.working.behavior.raise_on_focus = on;
            Cmd::None
        }
        Msg::HideBarOnFullscreenToggled(on) => {
            m.model.working.behavior.hide_bar_on_fullscreen = on;
            Cmd::None
        }
        Msg::SnapEnabledToggled(on) => {
            m.model.working.behavior.snap_enabled = on;
            Cmd::None
        }
        Msg::SnapGapChanged(v) => {
            m.model.working.appearance.snap_gap = crate::pages::appearance::spin_px(v);
            Cmd::None
        }
```

`Msg::SnapGapChanged` is declared in contract §2.2's Appearance group; the control that emits it lives on the Behavior page (spec §5.4, contract §2.3's id list, deviation P2-D10) and the field it writes stays in `icedtea_contract::Appearance`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: PASS — 16 tests, 0 failed.

- [ ] **Step 5: Mutation check**

Change `Msg::SnapEnabledToggled(on)`'s body to write `m.model.working.behavior.raise_on_focus = on;`.
Run: `cargo test -p icedtea-settings --lib app::tests::each_behavior_switch_writes_its_own_field`
Expected: FAIL — `the other two switches are untouched`.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/app.rs
git commit -m "feat(settings): Behavior update arms

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 7: `pages::appearance::view`

**Files:**
- Modify: `settings/src/pages/appearance.rs`
- Test: `settings/src/pages/appearance.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes:
  - `icedtea_ui::view::builders as w` — `box_`, `grid`, `label`, `button`, `button_from`, `entry`, `drop_down`, `spin_button`, `drawing_area`, `picture`; the traits `GridExt`, `LabelExt`, `EntryExt`, `DropDownExt`, `SpinButtonExt`, `DrawingAreaExt`.
  - `icedtea_ui::view::{Cmd, View}`, `icedtea_ui::layout::Align`, `icedtea_ui::widgets::types::Orientation`, `icedtea_ui::paint::fill_paint`, `icedtea_ui::widgets::color_dialog::ColorDialogC` (for `pack`/`default_palette` only), `icedtea_ui::css::value::Rgba`.
  - Task 4's `crate::app::{ColorSlot, Msg, SettingsModel}`; P1's `hex_to_rgba`/`hex_to_packed`; Task 1's `SPIN_MAX_PX`.
- Produces:
  ```rust
  pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>;
  pub fn swatch(rgba: icedtea_ui::css::value::Rgba) -> icedtea_ui::view::View<crate::app::Msg>;
  ```
  Widget ids, normative for every P2–P6 gate: `appearance_bar_position`, `appearance_bar_height`, `appearance_corner_radius`, `appearance_background`, `appearance_foreground`, `appearance_accent`, `appearance_wallpaper_entry`, `appearance_wallpaper_browse`, `appearance_wallpaper_clear`, `appearance_wallpaper_status`, `appearance_picker`, `appearance_picker_close`, `appearance_swatch_<i>`.

- [ ] **Step 1: Write the failing test**

Add to `settings/src/pages/appearance.rs`'s `mod tests`:

```rust
    use icedtea_ui::anim::{Clock, ManualClock};
    use icedtea_ui::css::cascade::CompiledSheet;
    use icedtea_ui::icons::IconTheme;
    use icedtea_ui::text::FontDatabase;
    use icedtea_ui::view::App;

    /// Lay the page out with no compositor and collect every node id.
    fn ids_of(m: crate::app::SettingsModel) -> Vec<String> {
        let sheet = CompiledSheet::compile(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
        let clock: std::rc::Rc<dyn Clock> = std::rc::Rc::new(ManualClock::new());
        let probe = App::new(m, crate::app::update, view)
            .probe(
                (480, 420),
                sheet,
                FontDatabase::new(),
                IconTheme::from_env(),
                clock,
            )
            .expect("the appearance page lays out");
        probe
            .root()
            .descendants()
            .filter_map(|n| n.id().map(|id| id.to_string()))
            .collect()
    }

    #[test]
    fn the_page_carries_every_id_its_gates_address() {
        let (m, _inbox) = crate::app::tests::test_model();
        let ids = ids_of(m);
        for id in [
            "appearance_bar_position",
            "appearance_bar_height",
            "appearance_corner_radius",
            "appearance_background",
            "appearance_foreground",
            "appearance_accent",
            "appearance_wallpaper_entry",
            "appearance_wallpaper_browse",
            "appearance_wallpaper_clear",
        ] {
            assert!(ids.contains(&id.to_string()), "{id} is missing from {ids:?}");
        }
        assert!(
            !ids.iter().any(|id| id.starts_with("appearance_swatch_")),
            "the palette panel is closed until a slot is opened"
        );
    }

    #[test]
    fn opening_a_slot_reveals_the_palette_panel() {
        let (mut m, _inbox) = crate::app::tests::test_model();
        m.color_picker = Some(crate::app::ColorSlot::Foreground);
        let ids = ids_of(m);
        assert!(ids.contains(&"appearance_picker".to_string()));
        assert!(ids.contains(&"appearance_picker_close".to_string()));
        assert!(
            ids.contains(&"appearance_swatch_0".to_string()),
            "one button per palette entry"
        );
    }

    #[test]
    fn the_wallpaper_status_row_shows_the_error_when_there_is_one() {
        let (mut m, _inbox) = crate::app::tests::test_model();
        m.wallpaper_error = Some("No such file: /nope.png".to_string());
        let ids = ids_of(m);
        assert!(ids.contains(&"appearance_wallpaper_status".to_string()));
    }
```

Make `mod tests` in `settings/src/app.rs` reachable from here by declaring it `#[cfg(test)] pub(crate) mod tests` and `test_model` `pub(crate) fn` (Task 4 already wrote the function as `pub(crate)`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-settings --lib pages::appearance::tests`
Expected: FAIL to compile — `cannot find function 'view' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `settings/src/pages/appearance.rs`:

```rust
use icedtea_ui::css::value::Rgba;
use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{
    self as w, DrawingAreaExt, DropDownExt, EntryExt, GridExt, LabelExt, SpinButtonExt,
};
use icedtea_ui::widgets::color_dialog::ColorDialogC;
use icedtea_ui::widgets::types::Orientation;

use crate::app::{ColorSlot, Msg, SettingsModel};
use crate::model::BAR_POSITIONS;

/// How wide and tall a colour swatch draws.
const SWATCH_W: i32 = 40;
const SWATCH_H: i32 = 22;

/// A flat rectangle of `rgba`.
///
/// Deviation P2-D11: `color_dialog_button`'s own chrome lives on a subnode
/// that never gets a taffy allocation, so `ColorDialogButtonC::on_event`'s
/// `local_rect` is always `None` and the widget cannot be clicked in a live
/// window (`ui/src/widgets/scrollbar.rs:309-317` documents the same shape).
/// A `drawing_area` is an ordinary leaf with a real allocation, a real probe
/// point, and P0's three-argument `Prop::Draw`.
#[must_use]
pub fn swatch(rgba: Rgba) -> View<Msg> {
    w::drawing_area(move |canvas, rect, _cx| {
        canvas.draw_rect(&rect.to_skia(), &icedtea_ui::paint::fill_paint(rgba));
    })
    .content_width(SWATCH_W)
    .content_height(SWATCH_H)
}

/// One labelled grid row: the label in column 0, the control in column 1.
fn row(index: u16, text: &str, control: View<Msg>) -> [View<Msg>; 2] {
    [
        w::label(text).halign(Align::Start).at(0, index),
        control.at(1, index),
    ]
}

/// One row whose control spans both columns (the wallpaper buttons and the
/// palette panel, which have no label of their own).
fn wide(index: u16, control: View<Msg>) -> [View<Msg>; 1] {
    [control.at(0, index).span(2, 1)]
}

/// The palette panel, shown while `m.color_picker` names a slot.
fn picker(slot: ColorSlot) -> View<Msg> {
    let palette = ColorDialogC::default_palette();
    let columns = 5u16;
    let buttons = palette.iter().enumerate().map(|(index, rgba)| {
        let packed = ColorDialogC::pack(*rgba);
        let msg = match slot {
            ColorSlot::Background => Msg::BackgroundPicked(packed),
            ColorSlot::Foreground => Msg::ForegroundPicked(packed),
            ColorSlot::Accent => Msg::AccentPicked(packed),
        };
        w::button_from(swatch(*rgba))
            .id(&format!("appearance_swatch_{index}"))
            .key(index)
            .at(index as u16 % columns, index as u16 / columns)
            .on_click(msg)
    });
    w::box_(
        Orientation::Vertical,
        [
            w::grid(buttons).row_spacing(4).column_spacing(4),
            w::button("Close")
                .id("appearance_picker_close")
                .halign(Align::Start)
                .on_click(Msg::ColorPickerClosed),
        ],
    )
    .id("appearance_picker")
}

/// The Appearance page.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let a = &m.model.working.appearance;
    let position = BAR_POSITIONS
        .iter()
        .position(|p| *p == a.bar_position)
        .unwrap_or(0);

    let mut children: Vec<View<Msg>> = Vec::new();
    children.extend(row(
        0,
        "Bar position",
        w::drop_down(&BAR_POSITIONS)
            .selected(position)
            .id("appearance_bar_position")
            .on_selected(Msg::BarPositionSelected),
    ));
    children.extend(row(
        1,
        "Bar height",
        w::spin_button(f64::from(a.bar_height), 0.0, f64::from(SPIN_MAX_PX))
            .step(1.0)
            .id("appearance_bar_height")
            .on_value_changed(Msg::BarHeightChanged),
    ));
    children.extend(row(
        2,
        "Corner radius",
        w::spin_button(f64::from(a.corner_radius), 0.0, f64::from(SPIN_MAX_PX))
            .step(1.0)
            .id("appearance_corner_radius")
            .on_value_changed(Msg::CornerRadiusChanged),
    ));
    for (index, (text, hex, slot, id)) in [
        (
            "Background",
            a.palette.background.as_str(),
            ColorSlot::Background,
            "appearance_background",
        ),
        (
            "Foreground",
            a.palette.foreground.as_str(),
            ColorSlot::Foreground,
            "appearance_foreground",
        ),
        (
            "Accent",
            a.palette.accent.as_str(),
            ColorSlot::Accent,
            "appearance_accent",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        children.extend(row(
            3 + index as u16,
            text,
            w::button_from(swatch(hex_to_rgba(hex)))
                .id(id)
                .halign(Align::Start)
                .on_click(Msg::ColorPickerOpened(slot)),
        ));
    }
    children.extend(row(
        6,
        "Wallpaper",
        w::entry(&m.wallpaper_text)
            .placeholder("/path/to/image.png")
            .id("appearance_wallpaper_entry")
            .hexpand(true)
            .on_change(|s| Msg::WallpaperEdited(s.to_string())),
    ));
    children.extend(wide(
        7,
        w::box_(
            Orientation::Horizontal,
            [
                w::button("Choose\u{2026}")
                    .id("appearance_wallpaper_browse")
                    .sensitive(m.portal_available)
                    .on_click(Msg::WallpaperBrowse),
                w::button("Clear")
                    .id("appearance_wallpaper_clear")
                    .on_click(Msg::WallpaperCleared),
            ],
        )
        .halign(Align::Start),
    ));
    children.extend(wide(8, wallpaper_status(m)));
    if let Some(slot) = m.color_picker {
        children.extend(wide(9, picker(slot)));
    }

    w::grid(children)
        .row_spacing(10)
        .column_spacing(16)
        .margin(16, 16, 16, 16)
        .id("appearance")
}

/// The row under the wallpaper buttons: the error, or a preview of the image
/// the model currently holds, or nothing to say.
fn wallpaper_status(m: &SettingsModel) -> View<Msg> {
    if let Some(error) = &m.wallpaper_error {
        return w::label(error)
            .halign(Align::Start)
            .classes(&["error"])
            .id("appearance_wallpaper_status");
    }
    match &m.model.working.appearance.wallpaper {
        Some(path) => w::picture(std::path::Path::new(path))
            .height_request(72)
            .halign(Align::Start)
            .id("appearance_wallpaper_status"),
        None => w::label("No wallpaper")
            .halign(Align::Start)
            .id("appearance_wallpaper_status"),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::appearance::tests`
Expected: PASS — 10 tests, 0 failed.

- [ ] **Step 5: Mutation check**

Change `if let Some(slot) = m.color_picker` to `if false` and drop the binding with `let _ = m.color_picker;`.
Run: `cargo test -p icedtea-settings --lib pages::appearance::tests::opening_a_slot_reveals_the_palette_panel`
Expected: FAIL — `appearance_picker` is missing from the id list.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/src/pages/appearance.rs settings/src/app.rs
git commit -m "feat(settings): the Appearance page view

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 8: `pages::behavior::view`

**Files:**
- Rewrite: `settings/src/pages/behavior.rs`
- Test: `settings/src/pages/behavior.rs` (a new `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `icedtea_ui::view::builders as w` (`grid`, `label`, `switch`, `spin_button`), traits `GridExt`, `SwitchExt`, `SpinButtonExt`; Task 4's `crate::app::{Msg, SettingsModel}`; Task 1's `SPIN_MAX_PX`; `icedtea_ui::layout::Align`.
- Produces:
  ```rust
  pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>;
  ```
  Widget ids, normative: `behavior_raise_on_focus`, `behavior_hide_bar_on_fullscreen`, `behavior_snap_enabled`, `behavior_snap_gap`.

- [ ] **Step 1: Write the failing test**

Create the test module at the bottom of `settings/src/pages/behavior.rs`:

```rust
#[cfg(test)]
mod tests {
    use icedtea_ui::anim::{Clock, ManualClock};
    use icedtea_ui::css::cascade::CompiledSheet;
    use icedtea_ui::icons::IconTheme;
    use icedtea_ui::text::FontDatabase;
    use icedtea_ui::view::App;

    use super::*;

    fn ids_of(m: crate::app::SettingsModel) -> Vec<String> {
        let sheet = CompiledSheet::compile(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
        let clock: std::rc::Rc<dyn Clock> = std::rc::Rc::new(ManualClock::new());
        let probe = App::new(m, crate::app::update, view)
            .probe(
                (480, 420),
                sheet,
                FontDatabase::new(),
                IconTheme::from_env(),
                clock,
            )
            .expect("the behavior page lays out");
        probe
            .root()
            .descendants()
            .filter_map(|n| n.id().map(|id| id.to_string()))
            .collect()
    }

    #[test]
    fn the_page_carries_every_id_its_gates_address() {
        let (m, _inbox) = crate::app::tests::test_model();
        let ids = ids_of(m);
        for id in [
            "behavior_raise_on_focus",
            "behavior_hide_bar_on_fullscreen",
            "behavior_snap_enabled",
            "behavior_snap_gap",
        ] {
            assert!(ids.contains(&id.to_string()), "{id} is missing from {ids:?}");
        }
    }

    #[test]
    fn the_page_lays_out_whatever_the_model_says() {
        // Both extremes of every control: a page that only lays out for the
        // default config is a page that will crash on a real one.
        let (mut m, _inbox) = crate::app::tests::test_model();
        m.model.working.behavior.raise_on_focus = true;
        m.model.working.behavior.hide_bar_on_fullscreen = true;
        m.model.working.behavior.snap_enabled = true;
        m.model.working.appearance.snap_gap = crate::pages::appearance::SPIN_MAX_PX;
        assert!(!ids_of(m).is_empty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-settings --lib pages::behavior`
Expected: FAIL to compile — the file still holds the GTK `build(ctx: Ctx) -> Page`, so `use gtk4::…` cannot resolve after P1 removed the dependency (or, if P1 already emptied the module, `cannot find function 'view' in this scope`).

- [ ] **Step 3: Write the implementation**

Replace the whole of `settings/src/pages/behavior.rs` above the test module with:

```rust
//! The Behavior page: three boolean switches and the snap gap.
//!
//! Every control is a pure function of `model.working`; there is no populate
//! pass and therefore no populate-vs-write-back guard (contract §2.2). The
//! snap-gap spin button lives here rather than on Appearance (spec §5.4) even
//! though the value it writes is `working.appearance.snap_gap` — the config
//! layout is not part of this migration.

use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{self as w, GridExt, SpinButtonExt, SwitchExt};

use crate::app::{Msg, SettingsModel};
use crate::pages::appearance::SPIN_MAX_PX;

/// One labelled grid row.
fn row(index: u16, text: &str, control: View<Msg>) -> [View<Msg>; 2] {
    [
        w::label(text).halign(Align::Start).at(0, index),
        control.halign(Align::Start).at(1, index),
    ]
}

/// The Behavior page.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let b = &m.model.working.behavior;
    let children = [
        row(
            0,
            "Raise on focus",
            w::switch(b.raise_on_focus)
                .id("behavior_raise_on_focus")
                .on_toggle(Msg::RaiseOnFocusToggled),
        ),
        row(
            1,
            "Hide bar on fullscreen",
            w::switch(b.hide_bar_on_fullscreen)
                .id("behavior_hide_bar_on_fullscreen")
                .on_toggle(Msg::HideBarOnFullscreenToggled),
        ),
        row(
            2,
            "Snap enabled",
            w::switch(b.snap_enabled)
                .id("behavior_snap_enabled")
                .on_toggle(Msg::SnapEnabledToggled),
        ),
        row(
            3,
            "Snap gap",
            w::spin_button(
                f64::from(m.model.working.appearance.snap_gap),
                0.0,
                f64::from(SPIN_MAX_PX),
            )
            .step(1.0)
            .id("behavior_snap_gap")
            .on_value_changed(Msg::SnapGapChanged),
        ),
    ];

    w::grid(children.into_iter().flatten())
        .row_spacing(10)
        .column_spacing(16)
        .margin(16, 16, 16, 16)
        .id("behavior")
}
```

Note the `w::label` call needs `LabelExt` in scope only for `.ellipsize`/`.wrap`, which this page does not use; `halign` is inherent on `View`, so the import list above is complete.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::behavior`
Expected: PASS — 2 tests, 0 failed.

- [ ] **Step 5: Verify the page is wired into the stack and the crate still builds**

Run: `cargo test -p icedtea-settings --lib`
Expected: PASS — every module's tests, including P1's moved suites (8 + 10 + 1 + 12 + 6 + 6), 0 failed.
Run: `grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add settings/src/pages/behavior.rs
git commit -m "feat(settings): the Behavior page view

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 9: Harness scaffolding — spawn the settings binary and read it back

**Files:**
- Create (or extend; P2-D2): `settings/tests/support/mod.rs`
- Modify: `settings/src/main.rs`

**Interfaces:**
- Consumes:
  - From P0 (M5-D9): the settings binary writes `probe <label> <x> <y>` and `alloc <id> <x> <y> <w> <h>` lines to `$ICEDTEA_PROBE_REPORT`, one block per frame in which the tree changed, for every laid-out node (`probe`) and every node carrying a `View::id` (`alloc`), in window-surface coordinates.
  - From P1: `settings/src/main.rs`'s window bootstrap and its `SettingsModel::new`.
  - `icedtea_harness::{Compositor, ScreencopyClient, VirtualPointerClient, CapturedFrame}`; `icedtea_ui::shm::pixel_rgb`; `icedtea_ui::wayland::BTN_LEFT`.
- Produces:
  ```rust
  pub const TITLE_BAR_HEIGHT: i32;                 // 28
  pub const SETTINGS_APP_ID: &str;                 // "org.icedtea.Settings"
  pub const SCREENCOPY_TOLERANCE: u8;              // 12
  pub struct Reaper(pub std::process::Child);
  pub struct EntryAllocation { pub id: String, pub x: f32, pub y: f32, pub width: f32, pub height: f32 }
  pub struct ProbePoint { pub label: String, pub x: i32, pub y: i32 }

  pub fn seeded_config_dir(edit: impl FnOnce(&mut icedtea_config::Config)) -> tempfile::TempDir;
  pub fn spawn_settings(socket: &str, config_home: &std::path::Path, theme: &str, page: &str,
                        report: &std::path::Path) -> Reaper;
  pub fn wait_for_window(compositor: &icedtea_harness::Compositor)
      -> icedtea_contract::Rectangle;
  pub fn client_origin(geometry: icedtea_contract::Rectangle) -> (i32, i32);
  pub fn report_lines(path: &std::path::Path) -> Vec<String>;
  pub fn wait_for_prefix(path: &std::path::Path, prefix: &str, timeout: std::time::Duration)
      -> Option<String>;
  pub fn latest_allocations(path: &std::path::Path)
      -> std::collections::HashMap<String, EntryAllocation>;
  pub fn latest_probe_points(path: &std::path::Path) -> Vec<ProbePoint>;
  pub fn pixel_at(frame: &icedtea_harness::CapturedFrame, x: i32, y: i32) -> Option<(u8, u8, u8)>;
  pub fn matches(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool;
  pub fn dominant_colour(frame: &icedtea_harness::CapturedFrame,
                         rect: (i32, i32, i32, i32)) -> (u8, u8, u8);
  pub fn paints_something(frame: &icedtea_harness::CapturedFrame,
                          rect: (i32, i32, i32, i32), background: (u8, u8, u8)) -> bool;
  pub fn click(pointer: &mut icedtea_harness::VirtualPointerClient,
               screencopy: &mut icedtea_harness::ScreencopyClient, x: i32, y: i32);
  pub fn wait_pixel_change(screencopy: &mut icedtea_harness::ScreencopyClient,
                           x: i32, y: i32, before: (u8, u8, u8)) -> (u8, u8, u8);
  ```

- [ ] **Step 1: Write the failing test**

Create `settings/tests/support/mod.rs` with only its module doc and `#![allow(dead_code)]`, then create the first consumer as a smoke test at the bottom of the same file's future user — put it in a new `settings/tests/appearance.rs`:

```rust
//! The Appearance and Behavior page gates.

mod support;

use std::time::Duration;

use icedtea_harness::{Compositor, ScreencopyClient};
use support::{
    client_origin, latest_allocations, pixel_at, seeded_config_dir, spawn_settings,
    wait_for_prefix, wait_for_window,
};

#[test]
fn the_settings_window_maps_and_reports_its_widgets() {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|_| {});
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), "light", "appearance", report.path());

    wait_for_prefix(report.path(), "alloc appearance_bar_position ", Duration::from_secs(20))
        .expect("the appearance page never reported its allocations");
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let allocations = latest_allocations(report.path());
    let entry = allocations
        .get("appearance_wallpaper_entry")
        .expect("the wallpaper entry has an allocation");
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let frame = screencopy.capture();
    assert!(
        pixel_at(
            &frame,
            ox + entry.x as i32 + 2,
            oy + entry.y as i32 + 2
        )
        .is_some(),
        "the entry's own pixels are inside the captured output"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-settings --test appearance`
Expected: FAIL to compile — `cannot find function 'spawn_settings' in module 'support'` and five more of the same shape.

- [ ] **Step 3: Write the implementation**

`settings/tests/support/mod.rs`:

```rust
//! Shared scaffolding for `icedtea-settings`' harness-driven gates.
//!
//! Deliberately a sibling of `ui/tests/support/mod.rs` rather than a
//! dependency on it: that module is a test-only file of another crate and
//! Cargo cannot share it. The pieces repeated here are the small ones — the
//! screencopy tolerance, `pixel_at`, `paints_something` — and each carries the
//! same reasoning as its original.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// The compositor's server-side title bar (`compositor/src/decoration.rs`),
/// mirrored from `ui/tests/window_events.rs`.
pub const TITLE_BAR_HEIGHT: i32 = 28;

/// The app id `settings/src/main.rs` registers.
pub const SETTINGS_APP_ID: &str = "org.icedtea.Settings";

/// How far apart two channel bytes may be and still count as the same colour
/// on a screencopy capture, matching `ui/tests/support/mod.rs`'s constant and
/// its reasoning: the output goes through a format conversion the offscreen
/// gate does not.
pub const SCREENCOPY_TOLERANCE: u8 = 12;

/// How long a capture loop sleeps between frames.
const POLL: Duration = Duration::from_millis(25);

/// Kill the child on the way out however the test ends.
pub struct Reaper(pub Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One `alloc <id> <x> <y> <w> <h>` line.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryAllocation {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One `probe <label> <x> <y>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbePoint {
    pub label: String,
    pub x: i32,
    pub y: i32,
}

/// A temp `XDG_CONFIG_HOME` holding a config `edit` has had its way with.
///
/// `icedtea_config::default_db_path` reads `XDG_CONFIG_HOME`, so pointing the
/// child at this directory is all the isolation a gate needs — no new env knob.
#[must_use]
pub fn seeded_config_dir(edit: impl FnOnce(&mut icedtea_config::Config)) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("config dir");
    let db = dir.path().join("icedtea").join("config.redb");
    std::fs::create_dir_all(db.parent().expect("parent")).expect("create the config dir");
    let mut cfg = icedtea_config::default_config();
    edit(&mut cfg);
    icedtea_settings::model::apply(&cfg, &db).expect("seed the config db");
    dir
}

/// Spawn `icedtea-settings` against `socket`, reaped when the guard drops.
///
/// `theme` is `light`/`dark`/`hc`; `page` is a `PageId::name()` value.
#[must_use]
pub fn spawn_settings(
    socket: &str,
    config_home: &Path,
    theme: &str,
    page: &str,
    report: &Path,
) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_icedtea-settings"))
            .env("WAYLAND_DISPLAY", socket)
            .env("XDG_CONFIG_HOME", config_home)
            .env("ICEDTEA_UI_THEME", theme)
            .env("ICEDTEA_SETTINGS_PAGE", page)
            .env("ICEDTEA_PROBE_REPORT", report)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn icedtea-settings"),
    )
}

/// Poll the compositor's model until the settings window is in it.
///
/// # Panics
/// If the window never appears within twenty seconds — a mapped window is the
/// precondition of every gate here, not something to skip over.
#[must_use]
pub fn wait_for_window(compositor: &Compositor) -> icedtea_contract::Rectangle {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if let Some(window) = compositor
            .snapshot()
            .windows
            .into_iter()
            .find(|w| w.app_id == SETTINGS_APP_ID)
        {
            return window.geometry;
        }
        std::thread::sleep(POLL);
    }
    panic!("the settings window never entered the compositor's model");
}

/// Where the client area starts, in output coordinates: the frame's origin
/// plus the server-side title bar the compositor draws above it.
#[must_use]
pub fn client_origin(geometry: icedtea_contract::Rectangle) -> (i32, i32) {
    (geometry.x, geometry.y + TITLE_BAR_HEIGHT)
}

/// Every line the app has reported so far.
#[must_use]
pub fn report_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Poll the report until a line starting with `prefix` appears.
#[must_use]
pub fn wait_for_prefix(path: &Path, prefix: &str, timeout: Duration) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(line) = report_lines(path).into_iter().find(|l| l.starts_with(prefix)) {
            return Some(line);
        }
        std::thread::sleep(POLL);
    }
    None
}

/// The most recent allocation reported for each id.
///
/// The app appends a fresh block every time the tree changes, so the last
/// line for an id is the only one that describes what is on screen now.
#[must_use]
pub fn latest_allocations(path: &Path) -> HashMap<String, EntryAllocation> {
    let mut out = HashMap::new();
    for line in report_lines(path) {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("alloc") {
            continue;
        }
        let Some(id) = fields.next() else { continue };
        let numbers: Vec<f32> = fields.filter_map(|f| f.parse().ok()).collect();
        if numbers.len() != 4 {
            continue;
        }
        out.insert(
            id.to_string(),
            EntryAllocation {
                id: id.to_string(),
                x: numbers[0],
                y: numbers[1],
                width: numbers[2],
                height: numbers[3],
            },
        );
    }
    out
}

/// The most recent probe point reported for each label.
#[must_use]
pub fn latest_probe_points(path: &Path) -> Vec<ProbePoint> {
    let mut out: HashMap<String, ProbePoint> = HashMap::new();
    for line in report_lines(path) {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("probe") {
            continue;
        }
        let Some(label) = fields.next() else { continue };
        let numbers: Vec<i32> = fields.filter_map(|f| f.parse().ok()).collect();
        if numbers.len() != 2 {
            continue;
        }
        out.insert(
            label.to_string(),
            ProbePoint {
                label: label.to_string(),
                x: numbers[0],
                y: numbers[1],
            },
        );
    }
    let mut points: Vec<ProbePoint> = out.into_values().collect();
    points.sort_by(|a, b| a.label.cmp(&b.label));
    points
}

/// The RGB triple at `(x, y)`, or `None` outside the frame.
#[must_use]
pub fn pixel_at(frame: &CapturedFrame, x: i32, y: i32) -> Option<(u8, u8, u8)> {
    if x < 0 || y < 0 {
        return None;
    }
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x as u32, y as u32)
}

/// Whether two RGB triples match on every channel within the tolerance.
#[must_use]
pub fn matches(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let close = |x: u8, y: u8| i32::from(x).abs_diff(i32::from(y)) <= u32::from(SCREENCOPY_TOLERANCE);
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
}

/// The most common colour inside `rect`.
///
/// The page background, derived rather than sampled at a corner: a settings
/// page has chrome in every corner (a switcher above, a footer below), so
/// there is no fixed pixel that is reliably bare. The window background is by
/// construction the colour most of the client area is.
#[must_use]
pub fn dominant_colour(frame: &CapturedFrame, rect: (i32, i32, i32, i32)) -> (u8, u8, u8) {
    let (x, y, w, h) = rect;
    let mut counts: HashMap<(u8, u8, u8), usize> = HashMap::new();
    for py in y..y + h {
        for px in x..x + w {
            if let Some(colour) = pixel_at(frame, px, py) {
                *counts.entry(colour).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(colour, _)| colour)
        .expect("a non-empty client area")
}

/// Whether anything inside `rect` differs from `background`.
#[must_use]
pub fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return false;
    }
    (y..y + h).any(|py| {
        (x..x + w).any(|px| pixel_at(frame, px, py).is_some_and(|got| !matches(got, background)))
    })
}

/// Move, press and release the left button at `(x, y)`, letting the
/// compositor settle between phases.
///
/// The settle loop is `ui/tests/support/mod.rs`'s `move_to`: surface focus is
/// assigned asynchronously and a press sent in the same breath as the motion
/// races it.
pub fn click(pointer: &mut VirtualPointerClient, screencopy: &mut ScreencopyClient, x: i32, y: i32) {
    let (w, h) = (screencopy.capture().width, screencopy.capture().height);
    pointer.motion_absolute(f64::from(x), f64::from(y), w, h);
    pointer.frame();
    pointer.pump();
    for _ in 0..8 {
        std::thread::sleep(POLL);
        pointer.pump();
    }
    pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
    pointer.frame();
    pointer.pump();
    std::thread::sleep(POLL);
    pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
    pointer.frame();
    pointer.pump();
}

/// Capture until the pixel at `(x, y)` differs from `before`, or two seconds
/// pass. A generous complexity bound, not a timing pin.
pub fn wait_pixel_change(
    screencopy: &mut ScreencopyClient,
    x: i32,
    y: i32,
    before: (u8, u8, u8),
) -> (u8, u8, u8) {
    let started = Instant::now();
    let mut px = before;
    while started.elapsed() < Duration::from_secs(2) {
        let frame = screencopy.capture();
        px = pixel_at(&frame, x, y).unwrap_or(before);
        if !matches(px, before) {
            return px;
        }
        std::thread::sleep(POLL);
    }
    px
}
```

`settings/src/main.rs` gains the two knobs P2-D7 and P2-D8 name. Insert them where P1 builds the sheet and the initial model:

```rust
/// The bundled sheet this run themes against.
///
/// `light` (the default), `dark` or `hc`, exactly the three names
/// `gallery --theme` takes, so a gate can run one page in all three.
fn bundled_theme() -> &'static str {
    match std::env::var("ICEDTEA_UI_THEME").as_deref() {
        Ok("dark") => icedtea_ui::BUNDLED_ADWAITA_DARK,
        Ok("hc") => icedtea_ui::BUNDLED_ADWAITA_HC,
        _ => icedtea_ui::BUNDLED_ADWAITA_LIGHT,
    }
}

/// The page the window opens on.
///
/// A debug/test affordance with the same role as `gallery --widget`: it lets a
/// rest-state gate photograph one page without synthesising a switcher click.
/// An unknown name falls back to the first page rather than failing to start.
fn initial_page() -> icedtea_settings::pages::PageId {
    let Ok(name) = std::env::var("ICEDTEA_SETTINGS_PAGE") else {
        return icedtea_settings::pages::PageId::Appearance;
    };
    icedtea_settings::pages::PageId::ALL
        .into_iter()
        .find(|page| page.name() == name)
        .unwrap_or(icedtea_settings::pages::PageId::Appearance)
}
```

and use them: the sheet passed to `Window::open` is compiled from `bundled_theme()` (layered with `settings/style.css` exactly as P1 already does), and the `SettingsModel`'s `page` field is initialised to `initial_page()` instead of `PageId::Appearance`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p icedtea-settings --test appearance`
Expected: PASS — 1 test, 0 failed. (Takes tens of seconds: it boots a headless compositor and a real settings process.)

- [ ] **Step 5: Mutation check**

In `client_origin`, return `(geometry.x, geometry.y)` (drop the title bar).
Run: `cargo test -p icedtea-settings --test appearance`
Expected: still PASS — this smoke test only asserts the pixel is inside the frame, which the title-bar offset does not change. That is the honest reading: the mutation check for `client_origin` belongs to the rest-state gate in Task 10, which asserts on colours, and is recorded there.
Restore.

Instead mutate `latest_allocations` to keep the *first* line per id (`out.entry(..).or_insert(..)`).
Run: `cargo test -p icedtea-settings --test appearance`
Expected: still PASS on this smoke test; Task 10's gate is the one that fails. Restore, and note that Task 9 ships no load-bearing assertion of its own — its proof is that Tasks 10 and 11 pass.

- [ ] **Step 6: Commit**

```bash
git add settings/tests/support/mod.rs settings/tests/appearance.rs settings/src/main.rs
git commit -m "test(settings): harness scaffolding, theme and page env knobs

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 10: Rest-state gates for both pages, in three themes

**Files:**
- Modify: `settings/tests/appearance.rs`

**Interfaces:**
- Consumes: every function Task 9's support module produces; Task 7's and Task 8's normative id lists.
- Produces: no library API. Six `#[test]` functions:
  `appearance_page_paints_every_probe_point_at_rest_in_the_{light,dark,high_contrast}_theme`,
  `behavior_page_paints_every_probe_point_at_rest_in_the_{light,dark,high_contrast}_theme`.

- [ ] **Step 1: Write the failing tests**

Replace the body of `settings/tests/appearance.rs` below its `use` block (keep the Task 9 smoke test) with:

```rust
use std::collections::HashMap;

use icedtea_harness::CapturedFrame;
use support::{
    dominant_colour, latest_probe_points, paints_something, EntryAllocation,
};

/// Ids the Appearance page must have painted something into at rest.
///
/// Deliberately not "every id in the report": `appearance` and `appearance_picker`
/// are containers whose own paint is their children's, and asserting on them
/// would pass for free. These are the leaves a user can see and touch.
const APPEARANCE_REST_IDS: &[&str] = &[
    "appearance_bar_position",
    "appearance_bar_height",
    "appearance_corner_radius",
    "appearance_background",
    "appearance_foreground",
    "appearance_accent",
    "appearance_wallpaper_entry",
    "appearance_wallpaper_browse",
    "appearance_wallpaper_clear",
    "appearance_wallpaper_status",
];

/// The same, for Behavior.
const BEHAVIOR_REST_IDS: &[&str] = &[
    "behavior_raise_on_focus",
    "behavior_hide_bar_on_fullscreen",
    "behavior_snap_enabled",
    "behavior_snap_gap",
];

/// Every id in `ids` has a non-empty allocation and paints something that is
/// not the page background, in `theme`.
///
/// "Paints something" is the honest assertion, exactly as
/// `ui/tests/gallery_gate.rs` argues: a widget whose rectangle is entirely the
/// window background has not rendered, whatever its node tree says. Colours
/// are not pinned — `themed_button_offscreen.rs` is the file that pins those.
///
/// There are no `KNOWN_BLANK` exemptions here (spec §7): an app page that
/// cannot paint one of its own controls is a defect, not a backlog entry.
fn page_paints_at_rest(theme: &str, page: &str, ids: &[&str]) {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|cfg| {
        // A wallpaper the status row can render as a label rather than a
        // missing file, so the row is never accidentally empty.
        cfg.appearance.wallpaper = None;
    });
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), theme, page, report.path());

    let first = format!("alloc {} ", ids[0]);
    wait_for_prefix(report.path(), &first, Duration::from_secs(20))
        .unwrap_or_else(|| panic!("the {page} page never reported `{first}`"));
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let mut screencopy = ScreencopyClient::spawn(&socket);
    let frame = screencopy.capture();
    let background = dominant_colour(
        &frame,
        (
            ox,
            oy,
            geometry.width,
            geometry.height - support::TITLE_BAR_HEIGHT,
        ),
    );

    let allocations: HashMap<String, EntryAllocation> = latest_allocations(report.path());
    let points = latest_probe_points(report.path());
    let mut diagnostics: Vec<String> = Vec::new();

    for id in ids {
        let Some(alloc) = allocations.get(*id) else {
            diagnostics.push(format!("{id} reported no allocation at all"));
            continue;
        };
        let rect = (
            ox + alloc.x as i32,
            oy + alloc.y as i32,
            alloc.width as i32,
            alloc.height as i32,
        );
        if rect.2 <= 0 || rect.3 <= 0 {
            diagnostics.push(format!("{id} collapsed to {}x{}", rect.2, rect.3));
            continue;
        }
        if !paints_something(&frame, rect, background) {
            diagnostics.push(format!(
                "{id} painted nothing in the {theme} theme; its box is {rect:?}"
            ));
        }
        if let Some(point) = points.iter().find(|p| p.label == *id)
            && support::pixel_at(&frame, ox + point.x, oy + point.y).is_none()
        {
            diagnostics.push(format!(
                "{id}'s probe point ({}, {}) is outside the captured frame",
                point.x, point.y
            ));
        }
    }

    assert!(
        diagnostics.is_empty(),
        "the {page} page failed its rest-state gate in the {theme} theme:\n  {}",
        diagnostics.join("\n  ")
    );
}

#[test]
fn appearance_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    page_paints_at_rest("light", "appearance", APPEARANCE_REST_IDS);
}

#[test]
fn appearance_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    page_paints_at_rest("dark", "appearance", APPEARANCE_REST_IDS);
}

#[test]
fn appearance_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    page_paints_at_rest("hc", "appearance", APPEARANCE_REST_IDS);
}

#[test]
fn behavior_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    page_paints_at_rest("light", "behavior", BEHAVIOR_REST_IDS);
}

#[test]
fn behavior_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    page_paints_at_rest("dark", "behavior", BEHAVIOR_REST_IDS);
}

#[test]
fn behavior_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    page_paints_at_rest("hc", "behavior", BEHAVIOR_REST_IDS);
}
```

Extend the `use support::{...}` line at the top of the file to include `TITLE_BAR_HEIGHT` and `pixel_at` if they are not already imported through the `support::` path used above.

- [ ] **Step 2: Run the tests to verify they fail**

Before running, temporarily comment out the `.on_click(Msg::ColorPickerOpened(slot))` line and the `swatch(...)` argument in `pages/appearance.rs`'s palette rows, replacing the control with `w::label("")` — this reproduces the "a control that paints nothing" condition the gate exists to catch.
Run: `cargo test -p icedtea-settings --test appearance appearance_page_paints_every_probe_point_at_rest_in_the_light_theme`
Expected: FAIL — `appearance_background painted nothing in the light theme; its box is (…)`.
Restore `pages/appearance.rs`.

- [ ] **Step 3: Run the tests against the real implementation**

Run: `cargo test -p icedtea-settings --test appearance`
Expected: PASS — 7 tests (the Task 9 smoke test plus these six), 0 failed. Each boots its own compositor and settings process; the whole file takes minutes, which is the per-part screencopy budget the spec §7 allows for.

- [ ] **Step 4: Mutation check — the title-bar offset**

In `settings/tests/support/mod.rs`, change `client_origin` to `(geometry.x, geometry.y)`.
Run: `cargo test -p icedtea-settings --test appearance behavior_page_paints_every_probe_point_at_rest_in_the_light_theme`
Expected: FAIL — every id's rectangle is 28 px too high, so the topmost controls sample the title bar and the lowest sample past the window; the gate reports `behavior_snap_gap painted nothing`.
Restore and re-run; expected PASS.

- [ ] **Step 5: Mutation check — the allocation freshness rule**

In `latest_allocations`, change `out.insert(...)` to `out.entry(id.to_string()).or_insert(...)`.
Run: `cargo test -p icedtea-settings --test appearance appearance_page_paints_every_probe_point_at_rest_in_the_light_theme`
Expected: FAIL — the first reported block is the pre-configure layout at the window's requested size, so the boxes do not match what the compositor actually shows and at least one id reports `painted nothing`.
Restore and re-run; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add settings/tests/appearance.rs settings/tests/support/mod.rs
git commit -m "test(settings): rest-state gates for Appearance and Behavior, light/dark/hc

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 11: Interaction gates, the GTK test deletion, and the contract record

**Files:**
- Modify: `settings/tests/appearance.rs`
- Delete: `settings/tests/appearance_gtk.rs`
- Modify: `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6 only)

**Interfaces:**
- Consumes: Task 9's `click`, `wait_pixel_change`, `latest_allocations`, `client_origin`; Task 7's `appearance_background` / `appearance_swatch_<i>` ids; Task 8's `behavior_raise_on_focus` id; `icedtea_ui::widgets::color_dialog::ColorDialogC::default_palette`.
- Produces: no library API. Two `#[test]` functions: `a_colour_pick_changes_the_swatch`, `toggling_raise_on_focus_repaints_the_switch_and_the_footer`.

- [ ] **Step 1: Write the failing tests**

Append to `settings/tests/appearance.rs`:

```rust
use icedtea_harness::VirtualPointerClient;
use icedtea_ui::widgets::color_dialog::ColorDialogC;
use support::{click, matches as colour_matches, wait_pixel_change};

/// Spec §7's Appearance interaction gate: opening the picker and clicking a
/// palette entry repaints the row's swatch.
///
/// Contract deviation P2-D4 moves this out of P4's list — it is an Appearance
/// page gate and P4 may not touch that page.
///
/// Mutation check: delete `.on_click(msg)` from `picker`'s palette buttons in
/// `settings/src/pages/appearance.rs`; the swatch never changes colour and
/// this test fails with "the swatch still shows the old colour". Restore.
#[test]
fn a_colour_pick_changes_the_swatch() {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|cfg| {
        // A colour no palette entry uses, so "it changed" cannot be a
        // coincidence of the starting value.
        cfg.appearance.palette.background = "#010203".to_string();
    });
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), "light", "appearance", report.path());

    wait_for_prefix(
        report.path(),
        "alloc appearance_background ",
        Duration::from_secs(20),
    )
    .expect("the appearance page never reported its background swatch");
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let mut screencopy = ScreencopyClient::spawn(&socket);
    let mut pointer = VirtualPointerClient::spawn(&socket);

    let before_box = latest_allocations(report.path())
        .remove("appearance_background")
        .expect("the swatch has an allocation");
    let sample = (
        ox + before_box.x as i32 + before_box.width as i32 / 2,
        oy + before_box.y as i32 + before_box.height as i32 / 2,
    );
    let before = support::pixel_at(&screencopy.capture(), sample.0, sample.1)
        .expect("the swatch is on screen");
    assert!(
        colour_matches(before, (0x01, 0x02, 0x03)),
        "the swatch starts on the seeded colour, got {before:?}"
    );

    // Open the picker.
    click(&mut pointer, &mut screencopy, sample.0, sample.1);
    wait_for_prefix(
        report.path(),
        "alloc appearance_swatch_0 ",
        Duration::from_secs(10),
    )
    .expect("the palette panel never opened");

    // Pick the third palette entry: a mid-blue no theme uses for a control.
    let index = 2usize;
    let expected = ColorDialogC::default_palette()[index];
    let expected_rgb = (
        (expected.r * 255.0).round() as u8,
        (expected.g * 255.0).round() as u8,
        (expected.b * 255.0).round() as u8,
    );
    let target = latest_allocations(report.path())
        .remove(&format!("appearance_swatch_{index}"))
        .expect("the palette entry has an allocation");
    click(
        &mut pointer,
        &mut screencopy,
        ox + target.x as i32 + target.width as i32 / 2,
        oy + target.y as i32 + target.height as i32 / 2,
    );

    // The row swatch moved back to where it was before the panel opened, so
    // re-read its box rather than reusing the old one.
    wait_for_prefix(report.path(), "alloc appearance_background ", Duration::from_secs(10));
    let after_box = latest_allocations(report.path())
        .remove("appearance_background")
        .expect("the swatch still has an allocation");
    let after_sample = (
        ox + after_box.x as i32 + after_box.width as i32 / 2,
        oy + after_box.y as i32 + after_box.height as i32 / 2,
    );
    let after = wait_pixel_change(&mut screencopy, after_sample.0, after_sample.1, before);

    assert!(
        !colour_matches(after, before),
        "the swatch still shows the old colour {before:?}"
    );
    assert!(
        colour_matches(after, expected_rgb),
        "the swatch shows the picked palette entry: expected {expected_rgb:?}, got {after:?}"
    );
}

/// The Behavior page's only handler shape, end to end: a switch click reaches
/// `update`, the model goes dirty, and the footer recomputes.
///
/// Mutation check: delete `.on_toggle(Msg::RaiseOnFocusToggled)` from
/// `settings/src/pages/behavior.rs`; the switch still animates its own slider
/// but the footer never changes, and this test fails with "the footer never
/// reacted to the edit". Restore.
#[test]
fn toggling_raise_on_focus_repaints_the_switch_and_the_footer() {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|cfg| cfg.behavior.raise_on_focus = false);
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), "light", "behavior", report.path());

    wait_for_prefix(
        report.path(),
        "alloc behavior_raise_on_focus ",
        Duration::from_secs(20),
    )
    .expect("the behavior page never reported its first switch");
    wait_for_prefix(report.path(), "alloc apply ", Duration::from_secs(10))
        .expect("the footer never reported its Apply button");
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let mut screencopy = ScreencopyClient::spawn(&socket);
    let mut pointer = VirtualPointerClient::spawn(&socket);
    let allocations = latest_allocations(report.path());

    let apply = allocations.get("apply").expect("the Apply button");
    let apply_sample = (
        ox + apply.x as i32 + apply.width as i32 / 2,
        oy + apply.y as i32 + apply.height as i32 / 2,
    );
    let apply_before = support::pixel_at(&screencopy.capture(), apply_sample.0, apply_sample.1)
        .expect("Apply is on screen");

    let switch = allocations
        .get("behavior_raise_on_focus")
        .expect("the switch");
    click(
        &mut pointer,
        &mut screencopy,
        ox + switch.x as i32 + switch.width as i32 / 2,
        oy + switch.y as i32 + switch.height as i32 / 2,
    );

    let apply_after = wait_pixel_change(
        &mut screencopy,
        apply_sample.0,
        apply_sample.1,
        apply_before,
    );
    assert!(
        !colour_matches(apply_after, apply_before),
        "the footer never reacted to the edit: Apply is still {apply_before:?}"
    );
}
```

The footer's `apply` id is P1's (contract §2.3's normative id list), and its `.sensitive(dirty)` rule is what makes the pixel move.

- [ ] **Step 2: Run the tests to verify they fail**

First delete `.on_click(msg)` from `picker`'s palette buttons in `settings/src/pages/appearance.rs` and `.on_toggle(Msg::RaiseOnFocusToggled)` from `settings/src/pages/behavior.rs`.
Run: `cargo test -p icedtea-settings --test appearance a_colour_pick_changes_the_swatch toggling_raise_on_focus_repaints_the_switch_and_the_footer`
Expected: FAIL — `the swatch still shows the old colour (1, 2, 3)` and `the footer never reacted to the edit`.
Restore both handlers. These two deletions are the mutation checks the doc comments record.

- [ ] **Step 3: Run the tests against the real implementation**

Run: `cargo test -p icedtea-settings --test appearance`
Expected: PASS — 9 tests, 0 failed.

- [ ] **Step 4: Delete the GTK test and prove the crate is GTK-free**

```bash
git rm settings/tests/appearance_gtk.rs
```

Run: `grep -rn 'gtk4\|gtk4-layer-shell\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests`
Expected: no output.
Run: `cargo tree -p icedtea-settings -e normal -i gtk4`
Expected: `error: package ID specification 'gtk4' did not match any packages` (or the equivalent "package ID not found" message).

- [ ] **Step 5: Append the deviations to the contract's §6**

Append to `docs/superpowers/plans/2026-09-03-m5-part0-contract.md`'s §6, in the M3 §10 shape (**Carried out by / Added / Contract says / As shipped / Ruling**), one block per entry of this plan's "Contract deviations" section: `P2-D1` through `P2-D12`, each quoting the contract text it departs from and the ruling. The load-bearing ones to write out in full are **P2-D11** (the swatch/picker composition, with the `ui/src/widgets/scrollbar.rs:309-317` citation), **P2-D4** (the gate moving out of P4's list) and **P2-D8** (`ICEDTEA_SETTINGS_PAGE`). Also record, as a note under `M5-D2`, whether P0's `Inbox<Msg>` needed a public `try_recv` added in Task 3.

- [ ] **Step 6: Run the whole gate set and commit**

```bash
cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo test -p icedtea-settings
cargo test -p icedtea-ui --test gallery_gate --test interaction_gate --test window_events \
    --test widget_pixels --test counter_app --test node_trees --test reconcile_props
```
Expected: every command exits 0; `interaction_gate` reports 16 passed.

```bash
git add settings/tests/appearance.rs docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "test(settings): Appearance/Behavior interaction gates; drop the GTK page test

Records P2-D1..P2-D12 in the M5 contract's amendment ledger.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Self-Review

### 1. Spec coverage

| Spec / contract requirement | Task |
|---|---|
| Spec §5.4 Appearance: wallpaper `Entry` + portal picker | 1, 3, 5, 7 |
| Spec §5.4 Appearance: three `ColorDialogButton`s | 7 (as `button_from(drawing_area(..))`, deviation P2-D11), 4 |
| Spec §5.4 Appearance: bar position `DropDown` | 4, 7 |
| Spec §5.4 Appearance: height/radius `SpinButton`s | 1, 4, 7 |
| Spec §5.4 Behavior: three `Switch`es | 6, 8 |
| Spec §5.4 Behavior: snap-gap `SpinButton` | 6, 8 |
| Spec D5 portal `OpenFile`, validated `Entry` fallback with preview | 1, 2, 3, 5, 7 |
| Spec D8 outbound calls via `Cmd::Task`, never inside `update` | 5 (`browsing_with_a_portal_issues_a_task`) |
| Spec §7 rest-state screencopy per page, light/dark/hc, no `KNOWN_BLANK` exemptions | 10 |
| Spec §7 interaction gate "a colour pick changes a swatch" | 11 |
| Spec §7 "`appearance_gtk.rs` → not ported (§10 record)" | 11 |
| Contract §2.6 unit tests `wallpaper_validation_rejects_a_missing_file`, `…_an_unknown_extension`, `a_failed_picker_disables_browse_without_touching_the_model`, `portal_uri_decoding_handles_percent_escapes`, `packed_round_trips_through_the_color_dialog_packing` | 1 (two), 2 (one), 4/5 (one), 1 (one) |
| Contract §2.6 portal worker degrades on every named failure mode without panicking or hanging | 3 |
| Contract §2.3 normative widget ids for both pages | 7, 8 |
| Contract §5 "P2 must not touch other pages or `ui/`" | Verified: no task edits `workspaces.rs`, `keybindings.rs`, `displays*`, or anything under `ui/`. |
| Global: no `gtk4`/`glib`/`gio`/`gdk`/`pango`/`cairo` in `settings` | 8 Step 5, 11 Step 4 |

No spec §5.4 or contract §2.6 requirement is unassigned. Contract §2.6's line "Preview | `picture(...)` or a `label`" is Task 7's `wallpaper_status`.

**Deliberately not covered here, with a reason:** contract §2.8's `apply_reaches_reload_config_on_the_mock` (a footer gate belonging to P1/P4, not to either page P2 owns) and the `hex_rgba_round_trips` / `invalid_hex_falls_back_to_black_without_panicking` tests (contract §2.1 moves them with the code in P1; Task 1's run counts confirm they are still there).

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `implement later`, `fill in`, `add appropriate`, `handle edge cases`, `similar to Task`, and for code steps that describe without showing. None present. Every code step carries a compilable block; every command is a literal `cargo`/`git`/`grep` invocation with its expected output; every task ends in a commit whose message carries the trailer. The three "if P1 already shipped X" clauses (Tasks 1, 3, 9) each state exactly what to do in both cases and name the normative signature, rather than deferring the decision.

### 3. Type consistency vs the contract

- `validate_wallpaper(&str) -> Result<PathBuf, String>` — declared in Task 1, used in Task 5 (`set_wallpaper`) and Task 2 (`image_filters` reads `WALLPAPER_EXTENSIONS`, not this function). Consistent.
- `spin_px(f64) -> i32` — declared in Task 1, used in Task 4 (`BarHeightChanged`, `CornerRadiusChanged`) and Task 6 (`SnapGapChanged`). Consistent; `SPIN_MAX_PX: i32` is used in Tasks 7 and 8 as `f64::from(SPIN_MAX_PX)`.
- `packed_to_hex(f64) -> String` / `hex_to_packed(&str) -> f64` / `hex_to_rgba(&str) -> Rgba` — P1's, spelled identically in Tasks 4, 7 and the Task 1 test.
- `Msg::{BackgroundPicked, ForegroundPicked, AccentPicked}(f64)` — contract §2.2's shape, unchanged; the palette buttons produce the `f64` with `ColorDialogC::pack`, the update arms consume it with `packed_to_hex`. Round trip proven in Task 1.
- `Msg::WallpaperChosen(std::path::PathBuf)` — contract §2.2; Task 3's worker constructs it from `open_file`'s `Result<PathBuf, String>` and Task 5's arm converts with `path.display().to_string()`. Consistent.
- `portal::spawn(rx: crossbeam_channel::Receiver<PortalRequest>, tx: InboxSender<Msg>) -> JoinHandle<()>` — byte-identical to contract §2.6.
- `ColorSlot` is `Copy` (Task 4) so Task 7's `picker(slot)` takes it by value and the `move` closures in the palette buttons capture it without cloning; `Msg` stays `Send` because `ColorSlot` is a fieldless enum.
- `EntryAllocation` in Task 9 has field `id`, not `widget` — deliberately different from `ui/tests/support/mod.rs`'s struct of the same name, because these lines are keyed by `View::id` rather than by gallery entry; Tasks 10 and 11 use `.id`, `.x`, `.y`, `.width`, `.height` throughout. Consistent within this plan.
- `client_origin(geometry: Rectangle) -> (i32, i32)` is called with `wait_for_window`'s return value in Tasks 9, 10 and 11 — same type, same order.
