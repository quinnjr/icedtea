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
#![allow(dead_code)]

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
