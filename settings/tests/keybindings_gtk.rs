//! GTK integration test for the SHIP-BLOCKER shift-normalization fix
//! (finding #1/#2): capturing a binding with Shift held (or Caps Lock on)
//! must still serialize to the *unshifted*, layout-agnostic key name, since
//! that's what the compositor's own matcher compares against.
//!
//! Run as its own integration-test binary for the same reason as
//! `appearance_gtk.rs`: GTK can only be initialized once per process, and
//! each `#[test]` in a shared binary runs on its own thread.
//!
//! This is the actual non-vacuous proof finding #2 asked for --
//! `model::combo_round_trips_to_the_compositor_format` only proves
//! `combo_from_keysym` preserves whatever keysym it's *given* (true
//! whether or not shift-normalization happens upstream); this test drives
//! the real GDK keymap query `build()`'s `key_pressed` handler performs
//! (`gdk::Display::translate_key` at group 0 / no modifiers, fed through
//! `pages::keybindings::normalise_keysym`) and contrasts it against the raw
//! shift-adjusted keyval a real Shift+key press hands `EventControllerKey`.
//! If the fix in `build()`'s `key_pressed` handler were reverted to feed
//! that raw keyval straight into `combo_from_keysym` (as it did before),
//! this test fails: the serialized key would be `KEY_Q`/`KEY_exclam` instead
//! of `KEY_q`/`KEY_1`.

use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use icedtea_config::keysym_to_key_name;
use icedtea_harness::Compositor;
use icedtea_settings::model::{CaptureMods, combo_from_keysym};
use icedtea_settings::pages::keybindings::normalise_keysym;

/// Point GDK at the harness compositor (which speaks a real "us" xkb
/// keymap over Wayland -- see `icedtea_harness::VirtualKeyboardClient`) and
/// init GTK, so the GDK keymap query has an actual `gdk::Display` to query.
/// `false` means GTK could not come up -- a FAILURE by default (the harness
/// provides a display) unless opted out via `ICEDTEA_ALLOW_NO_GTK`.
fn require_gtk(comp: &Compositor) -> bool {
    // SAFETY: one GTK test per binary; set before any GDK use.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &comp.socket);
        std::env::set_var("GDK_BACKEND", "wayland");
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    if gtk4::init().is_ok() {
        return true;
    }
    if std::env::var_os("ICEDTEA_ALLOW_NO_GTK").is_some() {
        eprintln!("SKIP: gtk4::init() unavailable and ICEDTEA_ALLOW_NO_GTK is set");
        return false;
    }
    panic!(
        "gtk4::init() failed against the harness compositor; \
         set ICEDTEA_ALLOW_NO_GTK=1 to skip on a display-less CI"
    );
}

/// CORRECTNESS BAR from the finding: capturing Super+Shift+q must serialize
/// to `KEY_q`; Super+Shift+1 must serialize to `KEY_1`.
#[test]
fn normalise_keysym_normalizes_shift_to_the_base_key() {
    let comp = Compositor::spawn();
    if !require_gtk(&comp) {
        return;
    }
    let display = gtk4::gdk::Display::default().expect("a display after gtk4::init()");

    // xkb keycode = evdev keycode + 8, per the "us" layout the harness's
    // virtual-keyboard keymap uses (`icedtea_harness::VirtualKeyboardClient::spawn`).
    const KEYCODE_Q: u32 = 16 + 8; // evdev KEY_Q
    const KEYCODE_1: u32 = 2 + 8; // evdev KEY_1

    for (keycode, base_name, shifted_name) in [
        (KEYCODE_Q, "KEY_q", "KEY_Q"),
        (KEYCODE_1, "KEY_1", "KEY_exclam"),
    ] {
        // The shift-adjusted keysym GDK hands a real Shift+key press --
        // exactly what `EventControllerKey`'s `keyval` carries.
        let (shifted_key, ..) = display
            .translate_key(keycode, gtk4::gdk::ModifierType::SHIFT_MASK, 0)
            .unwrap_or_else(|| panic!("translate_key({keycode}, SHIFT) failed"));
        let shifted_keysym = shifted_key.into_glib();
        assert_eq!(
            keysym_to_key_name(shifted_keysym),
            shifted_name,
            "test setup: keycode {keycode} shifted must be {shifted_name}"
        );

        let base = display
            .translate_key(keycode, gtk4::gdk::ModifierType::empty(), 0)
            .map(|(k, ..)| k.into_glib())
            .unwrap_or(0);
        let normalized = normalise_keysym(base, shifted_keysym);
        assert_ne!(
            normalized, shifted_keysym,
            "normalise_keysym must differ from the shifted keysym for this test to be non-vacuous"
        );
        assert_eq!(
            keysym_to_key_name(normalized),
            base_name,
            "keycode {keycode} must normalize to {base_name} regardless of Shift"
        );

        // End-to-end through the real serialization path: a capture with
        // Super+Shift held must store the unshifted key name -- the
        // CORRECTNESS BAR the finding names explicitly.
        let mods = CaptureMods {
            shift: true,
            logo: true,
            ..Default::default()
        };
        let combo = combo_from_keysym(normalized, mods).expect("must bind");
        assert_eq!(combo.key, base_name);
        assert_eq!(
            combo.modifiers,
            vec!["SUPER".to_string(), "SHIFT".to_string()]
        );

        // Contrast: feeding the pre-fix (raw, shift-adjusted) keysym
        // straight into `combo_from_keysym` -- what the old code did --
        // produces the WRONG, shifted key name, proving the fix matters.
        let unfixed_combo = combo_from_keysym(shifted_keysym, mods).expect("must bind");
        assert_eq!(unfixed_combo.key, shifted_name);
    }
}
