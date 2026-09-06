//! P3's harness tests and rest-state gates for the Workspaces and
//! Keybindings pages.
//!
//! One integration binary for both pages (deviation P3-D4): contract §5
//! gives P3 exactly one new test file, and sharing it keeps the scaffolding
//! -- compositor, theme, spawned binary, probe-report parsing -- single
//! sourced without creating a `settings/tests/support/` module P2 may also
//! be creating.
//!
//! Everything here drives the **real** `icedtea-settings` binary under
//! `icedtea_harness::Compositor`. `settings/tests/live_apply.rs` is the one
//! test that must spawn the real `icedtea-compositor` instead (inherited
//! decision 11); nothing in this file touches it.
//!
//! Task 8 lands the scaffolding plus one test; several items below
//! (`KEY_ESC`, `KEY_B`, `PAGE_WORKSPACES`, `Settings::screencopy` /
//! `Settings::capture`) exist for Tasks 9 and 10's Workspaces and
//! rest-state gates in this same file (deviation P3-D4) and are unused
//! until then.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{
    CapturedFrame, Compositor, ScreencopyClient, VirtualKeyboardClient, VirtualPointerClient,
};

/// The compositor's server-side title bar (`compositor/src/decoration.rs`),
/// so window-surface coordinates can be mapped onto the output.
const TITLE_BAR_HEIGHT: i32 = 28;

/// Linux evdev keycodes.
const KEY_ESC: u32 = 1;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_A: u32 = 30;
const KEY_B: u32 = 48;

/// How long a report line -- or a fresh, non-hidden `alloc` -- may take to
/// appear. A complexity bound, not a timing pin.
///
/// Reconciliation: widened from the plan's original 20s. P2-D19 records a
/// debug-profile settings frame at ~18s; `wait_for_alloc`'s own
/// reconciliation (below) makes it reject a hidden page's `0 0`-sized rows,
/// so a caller waiting on a widget that only exists after a page switch is
/// really waiting through one full extra render, and 20s left that race
/// losing intermittently. `settings/tests/skeleton.rs`'s `SETTLE` budgets
/// 45s for the same class of wait; this is rounded up from there.
const REPORT_TIMEOUT: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(40);

/// How long the compositor may take to add the settings window to its
/// model after the client's first probe report -- generous for the same
/// reason `REPORT_TIMEOUT` is (P2-D19): the wlr `mapped` signal can trail
/// a debug-profile client's own first paint by a wide margin, and
/// `settings/tests/skeleton.rs`'s `wait_for_window` budgets 45s for the
/// same wait.
const WINDOW_MAP_TIMEOUT: Duration = Duration::from_secs(60);

/// `PageId::ALL`'s indices, as `select_page` takes them.
const PAGE_WORKSPACES: usize = 2;
const PAGE_KEYBINDINGS: usize = 3;

/// Kills the settings process when dropped, including on a test panic.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One compositor, one settings process, one pointer and one keyboard.
struct Settings {
    compositor: Compositor,
    screencopy: ScreencopyClient,
    _reaper: Reaper,
    report: tempfile::NamedTempFile,
    _theme: tempfile::NamedTempFile,
    _config: tempfile::TempDir,
    /// The window-surface origin on the output.
    origin: (i32, i32),
    output: (u32, u32),
    /// The window's client content area, `(width, height)` -- the frame's
    /// own size, minus the title bar. Used to reject an `alloc` rectangle
    /// that falls outside it (see [`Settings::alloc`]).
    content_size: (i32, i32),
    pointer: VirtualPointerClient,
    keyboard: VirtualKeyboardClient,
}

impl Settings {
    /// Boot everything and wait for the first painted frame.
    ///
    /// The keyboard is spawned **before** the client: the headless seat
    /// advertises its keyboard capability only once a device exists on it,
    /// so a client that binds `wl_seat` first never binds `wl_keyboard` and
    /// can never receive `enter` (`ui/tests/window_events.rs`'s
    /// `a_virtual_keyboard_types_into_the_entry` documents the same
    /// ordering).
    fn spawn(theme_css: &str) -> Settings {
        let compositor = Compositor::spawn();
        let socket = compositor.socket_path().to_string_lossy().to_string();
        let (out_w, out_h) = compositor.output_size();
        assert!(
            out_w >= 640 && out_h >= 520,
            "a 480x420 settings window plus its title bar needs at least a \
             640x520 output, got {out_w}x{out_h}"
        );

        let keyboard = VirtualKeyboardClient::spawn(&socket);
        let pointer = VirtualPointerClient::spawn(&socket);

        let mut theme = tempfile::NamedTempFile::new().expect("theme file");
        std::io::Write::write_all(&mut theme, theme_css.as_bytes()).expect("write the theme");
        std::io::Write::flush(&mut theme).expect("flush the theme");

        let config = tempfile::tempdir().expect("config dir");
        let report = tempfile::NamedTempFile::new().expect("report file");

        let reaper = Reaper(
            Command::new(env!("CARGO_BIN_EXE_icedtea-settings"))
                .env("WAYLAND_DISPLAY", &socket)
                // The harness compositor's socket lives under its own
                // private runtime dir (fix wave, `harness/src/lib.rs`'s
                // `runtime_dir`); a child that inherited the session's
                // instead would look for `socket` in the wrong directory.
                // The process env is rewritten too, so this is not the only
                // thing making that true, but `settings/tests/support/
                // mod.rs`'s own `spawn_settings`/`spawn_settings_process`
                // now pass it explicitly, and this matches.
                .env("XDG_RUNTIME_DIR", icedtea_harness::runtime_dir())
                .env("XDG_CONFIG_HOME", config.path())
                .env("ICEDTEA_UI_THEME", theme.path())
                .env("ICEDTEA_PROBE_REPORT", report.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to spawn icedtea-settings"),
        );

        let mut settings = Settings {
            compositor,
            screencopy: ScreencopyClient::spawn(&socket),
            _reaper: reaper,
            report,
            _theme: theme,
            _config: config,
            origin: (0, 0),
            output: (out_w as u32, out_h as u32),
            content_size: (0, 0),
            pointer,
            keyboard,
        };

        settings
            .wait_for_line("probe root ")
            .expect("icedtea-settings never wrote a probe batch");
        // The model row is only created on the wlr `mapped` signal, which
        // under this headless backend can trail the client's own layout
        // (and so its first probe report) by a visible amount --
        // `settings/tests/skeleton.rs`'s `wait_for_window` documents the
        // same lag. Reconciliation of the plan's single `.find`.
        let started = Instant::now();
        let window = loop {
            if let Some(window) = settings
                .compositor
                .snapshot()
                .windows
                .into_iter()
                .find(|w| w.app_id == "org.icedtea.Settings")
            {
                break window;
            }
            assert!(
                started.elapsed() < WINDOW_MAP_TIMEOUT,
                "the settings window never appeared in the compositor's model"
            );
            std::thread::sleep(POLL);
        };
        settings.origin = (window.geometry.x, window.geometry.y + TITLE_BAR_HEIGHT);
        settings.content_size = (
            window.geometry.width,
            window.geometry.height - TITLE_BAR_HEIGHT,
        );
        settings
    }

    /// Every line written so far.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.report.path())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The most recent frame's batch: everything from the last
    /// `probe root …` line onwards. Every batch begins with that line
    /// because `Window::probe_points` walks the tree root-first (M5-D9).
    fn batch(&self) -> Vec<String> {
        let lines = self.lines();
        let start = lines
            .iter()
            .rposition(|l| l.starts_with("probe root "))
            .unwrap_or(0);
        lines[start..].to_vec()
    }

    /// Poll until a line starting with `prefix` appears anywhere in the
    /// report.
    fn wait_for_line(&self, prefix: &str) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < REPORT_TIMEOUT {
            if let Some(line) = self.lines().into_iter().find(|l| l.starts_with(prefix)) {
                return Some(line);
            }
            std::thread::sleep(POLL);
        }
        None
    }

    /// How many lines so far start with `prefix`.
    ///
    /// Paired with [`Settings::wait_for_line_after`] so a caller can wait
    /// for the *next* occurrence of a repeatable line (e.g. a second
    /// `msg CaptureArmed("close")` from a re-arm) rather than
    /// [`Settings::wait_for_line`]'s first-ever match, which would return
    /// immediately on a stale line already in the report.
    fn count_lines(&self, prefix: &str) -> usize {
        self.lines()
            .iter()
            .filter(|l| l.starts_with(prefix))
            .count()
    }

    /// Poll until more than `after` lines starting with `prefix` exist,
    /// then return the `after`-th one (0-indexed among matches) -- i.e.
    /// the first occurrence *after* the `after` already seen by a prior
    /// [`Settings::count_lines`] call.
    fn wait_for_line_after(&self, prefix: &str, after: usize) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < REPORT_TIMEOUT {
            let matches: Vec<String> = self
                .lines()
                .into_iter()
                .filter(|l| l.starts_with(prefix))
                .collect();
            if matches.len() > after {
                return Some(matches[after].clone());
            }
            std::thread::sleep(POLL);
        }
        None
    }

    /// `alloc <id> x y w h` from the latest batch, if present.
    ///
    /// Reconciliation: only a **non-zero** rectangle counts. A page's
    /// widgets keep their `alloc` lines while hidden (P2-D17's
    /// `display:none` gives them `0 0` for width/height rather than
    /// omitting them), so the plan's original presence-only check returned
    /// stale zero-sized rectangles for a page that had not actually become
    /// current yet -- `#nav` is always visible so this never showed up
    /// there, but a callee-owned id on the Keybindings/Workspaces page
    /// needs the caller to wait past the switch, not just past the
    /// `display:none` node's own existence in the tree.
    fn alloc(&self, id: &str) -> Option<(i32, i32, i32, i32)> {
        let prefix = format!("alloc {id} ");
        let line = self.batch().into_iter().find(|l| l.starts_with(&prefix))?;
        let f: Vec<i32> = line
            .split_whitespace()
            .skip(2)
            .filter_map(|v| v.parse().ok())
            .collect();
        (f.len() == 4 && f[2] > 0 && f[3] > 0).then(|| (f[0], f[1], f[2], f[3]))
    }

    /// Poll until `id` has a non-zero rectangle in the latest batch, then
    /// return it.
    ///
    /// This alone is enough for a container used only as a readiness signal
    /// (`#nav`, `#keybindings_list`) -- their own box can legitimately be
    /// wider or taller than the window's content area (a `scrolled_window`
    /// reports its full, unclipped content size; `#nav`'s own linked-button
    /// row measures itself a little wider than the window here). A rect
    /// meant to be **clicked** needs the stronger check in
    /// [`Settings::wait_for_click_target`].
    fn wait_for_alloc(&self, id: &str) -> (i32, i32, i32, i32) {
        let started = Instant::now();
        while started.elapsed() < REPORT_TIMEOUT {
            if let Some(rect) = self.alloc(id) {
                return rect;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "#{id} never appeared in a probe batch; last batch was:\n{}",
            self.batch().join("\n")
        );
    }

    /// Poll until `id`'s rectangle is non-zero **and** its centre lies
    /// within the window's content area, then return it.
    ///
    /// Reconciliation: right after a page switch this harness reproducibly
    /// observed a Keybindings row's container land, for a while, at a
    /// rectangle placed **below the window's own visible height**
    /// (`settings/src/pages/keybindings.rs`'s row list repositioning itself
    /// once more after the switch, not merely resizing) before a later
    /// frame moved it to where it stays for good -- e.g. `(176, 405, 318,
    /// 718)` and then `(176, 34, 318, 718)` for the same id, in a 640x400
    /// window. The plan's original `wait_for_alloc`, used directly as a
    /// click target, would click the first non-zero rectangle and hit
    /// nothing: this is the same P2-D19-class extra-render lag `alloc`'s
    /// own doc records, one level up, at the one call site (`click_id`)
    /// that turns a rectangle into a screen coordinate.
    fn wait_for_click_target(&self, id: &str) -> (i32, i32, i32, i32) {
        let started = Instant::now();
        let (cw, ch) = self.content_size;
        while started.elapsed() < REPORT_TIMEOUT {
            if let Some(rect @ (x, y, w, h)) = self.alloc(id) {
                let (cx, cy) = (x + w / 2, y + h / 2);
                if cx >= 0 && cx < cw && cy >= 0 && cy < ch {
                    return rect;
                }
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "#{id} never had an on-screen rectangle in a probe batch; last batch was:\n{}",
            self.batch().join("\n")
        );
    }

    /// `(label, x, y)` for every probe point in the latest batch.
    fn probes(&self) -> Vec<(String, i32, i32)> {
        self.batch()
            .into_iter()
            .filter_map(|line| {
                let mut f = line.split_whitespace();
                (f.next()? == "probe").then_some(())?;
                let label = f.next()?.to_string();
                let x = f.next()?.parse().ok()?;
                let y = f.next()?.parse().ok()?;
                Some((label, x, y))
            })
            .collect()
    }

    /// Window-surface coordinates -> output coordinates.
    fn to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        (self.origin.0 + x, self.origin.1 + y)
    }

    /// Move, press and release the left button at an output coordinate.
    ///
    /// The eight settling round trips are `ui/tests/support/mod.rs`'s
    /// `Driver::move_to` rule: a button sent back to back with the motion
    /// that first put the pointer on a surface can reach the seat before
    /// focus is assigned, and `wlr_seat_pointer_notify_button` drops a
    /// button with no focused surface silently.
    ///
    /// Reconciliation: the plan's original shape sent one `motion_absolute`
    /// and then only pumped through the settle loop; against this harness
    /// that motion can be consumed before focus lands, and every following
    /// pump does nothing, so the button never finds a focused surface.
    /// `settings/tests/support/mod.rs`'s `click`/`click_fixed` re-send the
    /// motion on every settle iteration instead -- adopted verbatim here.
    fn click(&mut self, x: i32, y: i32) {
        for _ in 0..8 {
            self.pointer
                .motion_absolute(f64::from(x), f64::from(y), self.output.0, self.output.1);
            self.pointer.frame();
            self.pointer.pump();
            std::thread::sleep(Duration::from_millis(25));
        }
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
        self.pointer.frame();
        self.pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Click the centre of the widget with this id.
    fn click_id(&mut self, id: &str) {
        let (x, y, w, h) = self.wait_for_click_target(id);
        let (sx, sy) = self.to_screen(x + w / 2, y + h / 2);
        self.click(sx, sy);
    }

    /// Click the `index`-th `StackSwitcher` button, in `PageId::ALL` order.
    ///
    /// Derived, never hard-coded: the switcher's five `button.text-button`
    /// subnodes are the probe points labelled `button<N>` whose centres fall
    /// inside `#nav`'s allocation, and their `label` children are labelled
    /// `label<N>` so they never collide with the filter.
    fn select_page(&mut self, index: usize) {
        let (nx, ny, nw, nh) = self.wait_for_alloc("nav");
        let mut buttons: Vec<(i32, i32)> = self
            .probes()
            .into_iter()
            .filter(|(label, x, y)| {
                label.starts_with("button") && *x >= nx && *x < nx + nw && *y >= ny && *y < ny + nh
            })
            .map(|(_, x, y)| (x, y))
            .collect();
        buttons.sort_unstable();
        assert_eq!(
            buttons.len(),
            5,
            "the switcher must offer one button per PageId; found {buttons:?} in \
             nav ({nx},{ny},{nw},{nh})"
        );
        let (bx, by) = buttons[index];
        let (sx, sy) = self.to_screen(bx, by);
        self.click(sx, sy);
    }

    /// One screencopy frame.
    fn capture(&mut self) -> CapturedFrame {
        self.screencopy.capture()
    }
}

/// The GTK test this replaces (`settings/tests/keybindings_gtk.rs`) proved
/// the SHIP-BLOCKER shift-normalisation fix through GDK's own keymap. This
/// proves the same property through the real toolkit, the real compositor
/// and a real `zwp_virtual_keyboard_v1` press: capturing `Shift`+`a` must
/// store the *unshifted* `KEY_a`, because `icedtea_config::keys::
/// key_name_to_keysym` only ever encodes the unshifted keysym and a stored
/// `KEY_A` is a binding the compositor's `match_action` can never fire.
///
/// Mutation check: in `pages::keybindings::capture_key`, pass
/// `ev.keysym.raw()` where it passes `ev.base.raw()`; this test fails with
/// `binding close SHIFT KEY_A`. Restore. (Equivalently: make
/// `normalise_keysym` return `modified` unconditionally -- contract §5's
/// named P3 mutation check -- and this test fails the same way.)
///
/// The Shift the assertion demands reaches the client only because
/// `VirtualKeyboardClient` sends a `zwp_virtual_keyboard_v1.modifiers`
/// request alongside each `key` (P3-D10). wlroots' virtual-keyboard
/// implementation passes `update_state = false` to `wlr_keyboard_notify_key`,
/// so a `key` request alone never moves the compositor's xkb state and no
/// `wl_keyboard.modifiers` event is ever sent onwards -- the protocol makes
/// the injecting client the owner of modifier state. Without that request the
/// client's own keymap stays unshifted and this test fails with
/// `binding close - KEY_a` (a second, independent mutation check).
#[test]
fn shift_a_while_capturing_records_base_a_with_shift() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    // Arm the `close` row. Clicking the Set button also focuses *it*
    // (`ButtonC::on_event`'s `PointerDown` arm, P3-D8 -- not the row's
    // surrounding box, which never held focus in the plan's assumed
    // `GenericC` path: the row is `box_(...)` (`BoxC`), and neither
    // controller granted focus on click before that fix), which is what
    // puts a focus owner in the tree so the next key press is dispatched
    // at all.
    s.click_id("kb_set_close");
    // `click_id` only guarantees the pointer press/release reached the
    // compositor; it says nothing about whether `icedtea-settings`' own
    // `update` has *processed* the resulting `Msg::CaptureArmed` yet. The
    // window-root `.on_key` handler is armed from a `bool` baked into the
    // closure at `view()` time (P3-D1) -- re-baked only on the next render
    // reconcile makes *after* that message lands -- so a key sent before
    // this line is delivered to a still-unarmed handler and silently
    // dropped by `capture_key`'s own `!armed` gate. Root-caused with a
    // temporary `eprintln!` in `capture_key` (since removed): every key of
    // this test's own sequence reported `armed=false` until this wait was
    // added.
    s.wait_for_line("msg CaptureArmed(\"close\")")
        .expect("CaptureArmed(\"close\") never appeared in the report");

    s.keyboard.key_down(KEY_LEFTSHIFT);
    s.keyboard.key_press(KEY_A);
    s.keyboard.key_up(KEY_LEFTSHIFT);
    s.keyboard.pump();

    let line = s
        .wait_for_line("binding close ")
        .unwrap_or_else(|| panic!("no capture was recorded; report:\n{}", s.lines().join("\n")));
    assert_eq!(
        line, "binding close SHIFT KEY_a",
        "a Shift-held capture must store the unshifted key name"
    );
}

/// Escape while armed cancels: nothing is bound, and the row is disarmed --
/// proven by the positive control, a subsequent `b` that binds only after
/// the row is armed again.
///
/// Mutation check: drop `capture_key`'s Escape arm; the first assertion
/// fails, because Escape itself gets stored as the binding. Restore.
///
/// Reconciliation: each Set click is followed by a wait for its own
/// `msg CaptureArmed("close")` rather than firing the next key blind.
/// `shift_a_while_capturing_records_base_a_with_shift`'s own doc comment
/// names the underlying race (the root `.on_key` handler is armed from a
/// bool baked into the closure at `view()` time, re-baked only on the next
/// render reconcile): a key sent before a click's own arm has landed
/// reaches a still-unarmed handler and is silently dropped, which would
/// make both this test's absence assertions pass vacuously instead of
/// exercising Escape's own cancel path. The second wait uses
/// [`Settings::wait_for_line_after`] rather than [`Settings::wait_for_line`]
/// because by then a first `CaptureArmed` already sits in the report; the
/// plain, first-match wait would return on that stale line instead of
/// waiting for the re-arm.
#[test]
fn escape_cancels_a_capture() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    s.click_id("kb_set_close");
    s.wait_for_line("msg CaptureArmed(\"close\")")
        .unwrap_or_else(|| {
            panic!(
                "the first Set click never armed the capture; report:\n{}",
                s.lines().join("\n")
            )
        });
    s.keyboard.key_press(KEY_ESC);
    s.keyboard.pump();

    // The capture was cancelled, so the next key must not bind anything.
    s.keyboard.key_press(KEY_A);
    s.keyboard.pump();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !s.lines().iter().any(|l| l.starts_with("binding ")),
        "Escape must cancel; report:\n{}",
        s.lines().join("\n")
    );

    // Positive control: re-arm and the very next key does bind.
    let armed_before = s.count_lines("msg CaptureArmed(\"close\")");
    s.click_id("kb_set_close");
    s.wait_for_line_after("msg CaptureArmed(\"close\")", armed_before)
        .unwrap_or_else(|| {
            panic!(
                "re-arming after Escape never armed the capture; report:\n{}",
                s.lines().join("\n")
            )
        });
    s.keyboard.key_press(KEY_B);
    s.keyboard.pump();
    let line = s
        .wait_for_line("binding close ")
        .unwrap_or_else(|| panic!("re-arming did not work; report:\n{}", s.lines().join("\n")));
    assert_eq!(line, "binding close - KEY_b");
}

/// A lone Shift press mid-capture leaves the row armed and binds nothing --
/// the GTK behaviour verbatim -- and the real key that follows binds
/// normally, without a second click on Set.
///
/// Mutation check: make `apply_capture` return `true` when
/// `combo_from_keysym` returned `None`; the second assertion fails, because
/// the row disarms and the following `a` binds nothing. Restore.
///
/// Reconciliation: waits for its own `msg CaptureArmed("close")` before
/// sending Shift, for the same reason `escape_cancels_a_capture` does --
/// without it, a Shift sent before the click's own arm has reached the
/// root `.on_key` closure is silently dropped by `capture_key`'s `!armed`
/// gate, and the following `a` would never have been armed to begin with.
#[test]
fn a_lone_modifier_press_leaves_the_capture_armed_end_to_end() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    s.click_id("kb_set_close");
    s.wait_for_line("msg CaptureArmed(\"close\")")
        .unwrap_or_else(|| {
            panic!(
                "the Set click never armed the capture; report:\n{}",
                s.lines().join("\n")
            )
        });
    s.keyboard.key_press(KEY_LEFTSHIFT);
    s.keyboard.pump();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !s.lines().iter().any(|l| l.starts_with("binding ")),
        "a bare Shift must not become a binding; report:\n{}",
        s.lines().join("\n")
    );

    // Still armed: no second click on Set.
    s.keyboard.key_press(KEY_A);
    s.keyboard.pump();
    let line = s.wait_for_line("binding close ").unwrap_or_else(|| {
        panic!(
            "the capture did not stay armed; report:\n{}",
            s.lines().join("\n")
        )
    });
    assert_eq!(line, "binding close - KEY_a");
}

/// Leaving the page disarms, so a keystroke typed after the switch cannot
/// silently rebind the row a user left armed -- the GTK `connect_unmap` /
/// `EventControllerFocus` reset.
///
/// Mutation check: drop `m.capturing = None` from `Msg::PageSelected`; the
/// first assertion fails with `binding close - KEY_a`. Restore.
///
/// Reconciliation: both the first click and the re-arm wait for their own
/// `msg CaptureArmed("close")`, for the same race `escape_cancels_a_capture`
/// documents. The first click's wait is likely redundant in practice --
/// `select_page`/`wait_for_alloc("workspaces_add")` right after it already
/// forces a render past the switch -- but it costs nothing and keeps the
/// precondition for the mutation check (`capturing` genuinely `Some` when
/// `PageSelected` lands) explicit rather than incidental. The re-arm's wait
/// uses [`Settings::wait_for_line_after`], not the plain, first-match
/// [`Settings::wait_for_line`], because a first `CaptureArmed` is already in
/// the report by then.
#[test]
fn switching_pages_cancels_a_capture_end_to_end() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    s.click_id("kb_set_close");
    s.wait_for_line("msg CaptureArmed(\"close\")")
        .unwrap_or_else(|| {
            panic!(
                "the first Set click never armed the capture; report:\n{}",
                s.lines().join("\n")
            )
        });
    s.select_page(PAGE_WORKSPACES);
    s.wait_for_alloc("workspaces_add");

    s.keyboard.key_press(KEY_A);
    s.keyboard.pump();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !s.lines().iter().any(|l| l.starts_with("binding ")),
        "a page switch must disarm the capture; report:\n{}",
        s.lines().join("\n")
    );

    // Positive control: back on the page, arming still works.
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");
    let armed_before = s.count_lines("msg CaptureArmed(\"close\")");
    s.click_id("kb_set_close");
    s.wait_for_line_after("msg CaptureArmed(\"close\")", armed_before)
        .unwrap_or_else(|| {
            panic!(
                "re-arming after the switch never armed the capture; report:\n{}",
                s.lines().join("\n")
            )
        });
    s.keyboard.key_press(KEY_B);
    s.keyboard.pump();
    let line = s.wait_for_line("binding close ").unwrap_or_else(|| {
        panic!(
            "re-arming after the switch failed; report:\n{}",
            s.lines().join("\n")
        )
    });
    assert_eq!(line, "binding close - KEY_b");
}

/// The Workspaces page's Add and Remove reach the model: Add grows the row
/// set by one (a new `ws_row_<n>` appears in the probe batch) and Remove
/// shrinks it back. Read through the probe report's `alloc` lines, which is
/// the only thing about a running app this test can see.
///
/// Mutation check: make `remove_workspace` return before removing; the
/// second assertion fails. Restore.
#[test]
fn adding_and_removing_a_workspace_reaches_the_model() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_WORKSPACES);
    s.wait_for_alloc("workspaces_add");

    let rows_at_rest = s
        .batch()
        .iter()
        .filter(|l| l.starts_with("alloc ws_row_"))
        .count();
    assert!(
        rows_at_rest >= 1,
        "the default config has at least one workspace"
    );

    s.click_id("workspaces_add");
    s.wait_for_alloc(&format!("ws_row_{rows_at_rest}"));

    s.click_id(&format!("ws_remove_{rows_at_rest}"));
    let started = Instant::now();
    while started.elapsed() < REPORT_TIMEOUT {
        let rows = s
            .batch()
            .iter()
            .filter(|l| l.starts_with("alloc ws_row_"))
            .count();
        if rows == rows_at_rest {
            return;
        }
        std::thread::sleep(POLL);
    }
    panic!(
        "the row count never returned to {rows_at_rest}; last batch:\n{}",
        s.batch().join("\n")
    );
}

/// The pixel at an output coordinate, or `None` when it is outside the
/// frame.
///
/// Reconciliation: the brief names `icedtea_harness::pixel_at`, but the
/// harness exposes no such function -- `grep -n 'pub fn pixel_at'
/// harness/src/lib.rs` finds nothing. `CapturedFrame` (`harness/src/lib.rs`)
/// carries `width`/`height`/`stride`/`format`/`bytes`, not the brief's
/// assumed `data` field with a hard-coded Bgr888-3bpp layout, and
/// `settings/tests/support/mod.rs`'s own `pixel_at` (P2's proven rest-state
/// gate this task mirrors) already decodes a captured frame generically via
/// `icedtea_ui::shm::pixel_rgb`, which switches on `frame.format` rather
/// than assuming one. This file does not depend on that support module
/// (its own doc comment: one integration binary, single-sourced, no
/// `settings/tests/support/` dependency), so the same proven approach is
/// reimplemented locally here rather than duplicated by hand-rolling the
/// Bgr888 arithmetic the brief offered as a fallback.
fn pixel_at(frame: &CapturedFrame, x: i32, y: i32) -> Option<(u8, u8, u8)> {
    if x < 0 || y < 0 || x as u32 >= frame.width || y as u32 >= frame.height {
        return None;
    }
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x as u32, y as u32)
}

/// Whether anything inside `rect` (output coordinates) differs from
/// `background`.
///
/// A full scan of the border box, the rule `ui/tests/support/mod.rs`'s
/// `paints_something` settled on: a sparse grid misses a widget that drew
/// only a 1px border, and "painted nothing" reported for something that
/// plainly painted is worse than a slower gate.
fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return false;
    }
    (y..y + h)
        .any(|py| (x..x + w).any(|px| pixel_at(frame, px, py).is_some_and(|got| got != background)))
}

/// Run one page's rest-state gate in `theme_css`.
///
/// `page` is the switcher index; `prefixes` are the id prefixes that belong
/// to the page; `required` are ids that must always be visible; `floor` is
/// the smallest number of ids a healthy layout leaves fully inside the
/// window.
fn rest_state_gate(
    theme_css: &str,
    theme_name: &str,
    page: usize,
    prefixes: &[&str],
    required: &[&str],
    floor: usize,
) {
    let mut s = Settings::spawn(theme_css);
    s.select_page(page);
    s.wait_for_alloc(required[0]);
    // One more settle so the page's first frame is the one on screen, not
    // the switcher's own transition frame.
    std::thread::sleep(Duration::from_millis(300));

    // Reconciliation: the brief derives `(win_w, win_h)` from `#root`'s own
    // allocation, but this layout engine's `ScrolledWindow` reports its full,
    // unclipped content size as its natural height (documented on
    // `Settings::wait_for_alloc` above: "a `scrolled_window` reports its
    // full, unclipped content size"), and a plain `Box` above it has no
    // ceiling of its own -- only a `MinWidth`/`MinHeight` floor
    // (`ui/src/view/render.rs`'s `write_styles`, depth 0) -- so `#root`
    // balloons to its content's natural size on a page taller than the
    // window (confirmed empirically: `#root` reported `(0, 0, 670, 787)`
    // against a real `640x372` client area on the Keybindings page). Using
    // that inflated box as the clipping bound let rows genuinely below the
    // window through the filter instead of excluding them. `content_size`
    // -- the window's real client-area size from the compositor's own
    // geometry -- is the correct bound: it is what `wait_for_click_target`
    // already trusts for the same "is this actually on screen" question.
    let (win_w, win_h) = s.content_size;
    let frame = s.capture();
    // The page background: two pixels in from the client area's top-left
    // corner, which is the root box's own fill in every bundled theme.
    let (bx, by) = s.to_screen(2, 2);
    let background =
        pixel_at(&frame, bx, by).expect("the client area's corner is inside the captured frame");

    let batch = s.batch();
    let mut checked: Vec<String> = Vec::new();
    let mut blank: Vec<String> = Vec::new();
    for line in &batch {
        let Some(rest) = line.strip_prefix("alloc ") else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(id) = fields.next() else { continue };
        if !prefixes.iter().any(|p| id.starts_with(p)) {
            continue;
        }
        let nums: Vec<i32> = fields.filter_map(|v| v.parse().ok()).collect();
        if nums.len() != 4 {
            continue;
        }
        let (x, y, w, h) = (nums[0], nums[1], nums[2], nums[3]);
        // Skip what the ScrolledWindow legitimately clips away.
        //
        // Reconciliation: the page's own `ScrolledWindow` container (id
        // suffix `_list`) is a documented exception to the brief's full-rect
        // containment test (`Settings::wait_for_alloc`'s own doc: "a
        // `scrolled_window` reports its full, unclipped content size") --
        // `#keybindings_list` legitimately reports a box taller than the
        // window on this 19-row page, even though its top-left corner, and
        // the rows actually on screen inside it, paint normally. Requiring
        // full containment for that one id would make `#keybindings_list`
        // -- a required id -- impossible to ever check on this page.
        // Ordinary rows still need the full-rect test: a row whose *origin*
        // is on screen but whose bottom is not is exactly what the
        // ScrolledWindow legitimately clips away.
        let is_list_container = id.ends_with("_list");
        if x < 0 || y < 0 || (!is_list_container && (x + w > win_w || y + h > win_h)) {
            continue;
        }
        let (sx, sy) = s.to_screen(x, y);
        if !paints_something(&frame, (sx, sy, w, h), background) {
            blank.push(format!("#{id} ({x},{y},{w},{h})"));
        }
        checked.push(id.to_string());
    }

    assert!(
        blank.is_empty(),
        "these widgets painted nothing in the {theme_name} theme: {}",
        blank.join(", ")
    );
    assert!(
        checked.len() >= floor,
        "only {} of the page's widgets were inside the window in the {theme_name} \
         theme (expected at least {floor}): {checked:?}",
        checked.len()
    );
    for id in required {
        assert!(
            checked.iter().any(|c| c == id),
            "#{id} must be visible at rest in the {theme_name} theme; checked {checked:?}"
        );
    }

    // Every probe point the window reported is addressable: inside the
    // client rectangle and inside the frame.
    for (label, x, y) in s.probes() {
        if x < 0 || y < 0 || x >= win_w || y >= win_h {
            continue;
        }
        let (sx, sy) = s.to_screen(x, y);
        assert!(
            pixel_at(&frame, sx, sy).is_some(),
            "probe point {label} at ({x},{y}) is outside the captured frame in the \
             {theme_name} theme"
        );
    }
}

/// Ids that belong to the Workspaces page.
const WORKSPACES_PREFIXES: &[&str] = &["workspaces_", "ws_row_", "ws_name_", "ws_remove_"];
/// Ids that belong to the Keybindings page.
const KEYBINDINGS_PREFIXES: &[&str] = &["keybindings_", "kb_row_", "kb_combo_", "kb_set_"];

/// Every Workspaces widget paints at rest.
///
/// Mutation check: give `ws_name_<i>` `.visible(false)`; the `#ws_name_0
/// must be visible` assertion fails. Restore.
#[test]
fn workspaces_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT,
        "light",
        PAGE_WORKSPACES,
        WORKSPACES_PREFIXES,
        &[
            "workspaces_list",
            "workspaces_add",
            "ws_name_0",
            "ws_remove_0",
        ],
        6,
    );
}

#[test]
fn workspaces_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_DARK,
        "dark",
        PAGE_WORKSPACES,
        WORKSPACES_PREFIXES,
        &[
            "workspaces_list",
            "workspaces_add",
            "ws_name_0",
            "ws_remove_0",
        ],
        6,
    );
}

#[test]
fn workspaces_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_HC,
        "high contrast",
        PAGE_WORKSPACES,
        WORKSPACES_PREFIXES,
        &[
            "workspaces_list",
            "workspaces_add",
            "ws_name_0",
            "ws_remove_0",
        ],
        6,
    );
}

/// Every visible Keybindings widget paints at rest. The page scrolls, so
/// rows below the viewport are excluded by the clipping filter and the
/// floor plus the required ids keep the gate honest.
///
/// Mutation check: render the combo label with an empty string; `#kb_combo_
/// close` paints nothing and the gate fails. Restore.
#[test]
fn keybindings_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT,
        "light",
        PAGE_KEYBINDINGS,
        KEYBINDINGS_PREFIXES,
        &[
            "keybindings_list",
            "kb_row_close",
            "kb_combo_close",
            "kb_set_close",
        ],
        8,
    );
}

#[test]
fn keybindings_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_DARK,
        "dark",
        PAGE_KEYBINDINGS,
        KEYBINDINGS_PREFIXES,
        &[
            "keybindings_list",
            "kb_row_close",
            "kb_combo_close",
            "kb_set_close",
        ],
        8,
    );
}

#[test]
fn keybindings_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_HC,
        "high contrast",
        PAGE_KEYBINDINGS,
        KEYBINDINGS_PREFIXES,
        &[
            "keybindings_list",
            "kb_row_close",
            "kb_combo_close",
            "kb_set_close",
        ],
        8,
    );
}
