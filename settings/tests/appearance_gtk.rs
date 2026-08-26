//! GTK integration test for the Appearance page's populate-vs-write-back
//! guard (finding #3): loading a config value a widget cannot represent
//! must not silently mutate the working copy, even though the widget's
//! change signal still fires while `refresh()` sets it programmatically.
//!
//! Run as its own integration-test binary (rather than a `#[cfg(test)]`
//! module inside `src/pages/appearance.rs`) because GTK can only ever be
//! initialized once per process -- `gtk4::init()` panics with "Attempted to
//! initialize GTK from two different threads" if a second unit test in the
//! same test binary (each of which libtest runs on its own thread) also
//! calls it. Mirrors `shell/tests/shell_gtk.rs`'s own reasoning.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use icedtea_harness::Compositor;
use icedtea_settings::model::Model;
use icedtea_settings::pages::{Ctx, appearance};

/// Point GDK at the harness compositor and init GTK. `false` means GTK
/// could not come up -- a FAILURE by default (the harness provides a
/// display), not a silent pass. An operator on a genuinely display-less CI
/// opts out explicitly with `ICEDTEA_ALLOW_NO_GTK`.
fn gtk_init_against(comp: &Compositor) -> bool {
    // SAFETY: one GTK test per binary; set before any GDK use.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &comp.socket);
        std::env::set_var("GDK_BACKEND", "wayland");
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    gtk4::init().is_ok()
}

fn require_gtk(comp: &Compositor) -> bool {
    if gtk_init_against(comp) {
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

/// The regression proof for the `populating`-guard fix: loading a config
/// whose `bar_position` is a value the dropdown can't represent (not in
/// `BAR_POSITIONS`) makes `refresh()` fall the widget back to index 0
/// ("top") -- and that widget update fires `connect_selected_notify` the
/// same way a real user click would. Before the fix, that handler wrote the
/// widget's fallback straight back into `model.working`, silently replacing
/// "left" with "top" and leaving the model dirty. If the
/// `ctx.populating.get()` early-return in any of this page's `connect_*`
/// handlers were removed, this test fails.
#[test]
fn populate_never_writes_a_widget_fallback_back_into_the_model() {
    let comp = Compositor::spawn();
    if !require_gtk(&comp) {
        return;
    }

    // No `Application` attached -- `Ctx::window` only needs a plain
    // `ApplicationWindow` to use as `FileDialog`'s transient-for parent,
    // and attaching one before `GApplication::startup` has fired (which
    // this test never triggers) only produces a harmless GTK-CRITICAL log.
    let window = gtk4::ApplicationWindow::builder().build();

    let mut cfg = icedtea_config::default_config();
    // Not in `BAR_POSITIONS` -- forces the dropdown's fallback-to-index-0
    // path in `refresh()`.
    cfg.appearance.bar_position = "left".to_string();
    let model = Rc::new(RefCell::new(Model {
        working: cfg.clone(),
        saved: cfg,
    }));

    let ctx = Ctx {
        model: model.clone(),
        window,
        on_dirty: Rc::new(|| {}),
        populating: Rc::new(Cell::new(false)),
    };

    // `build()` runs an initial `refresh()` internally -- this alone
    // reproduces the bug (before the fix) without any further action.
    let page = appearance::build(ctx);
    // A second, explicit refresh (mirrors Revert) must be equally inert.
    page.refresh();

    assert_eq!(
        model.borrow().working.appearance.bar_position,
        "left",
        "populate must not overwrite an out-of-domain value the widget can't represent"
    );
    assert!(
        !model.borrow().is_dirty(),
        "populate must never mark the model dirty"
    );
}
