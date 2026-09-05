//! `icedtea-settings` — a config editor for icedtea, built on `icedtea-ui`.
//!
//! One `App<SettingsModel, Msg>` on one `Role::Toplevel` window. Two external
//! event sources reach the loop through the toolkit's ingress: the reload
//! worker posts on the inbox, and the Displays page's second Wayland
//! connection is registered with `Window::watch_fd` and mapped to messages by
//! `App::on_fd`.

use std::path::Path;

use icedtea_config::default_db_path;
use icedtea_settings::app::{SettingsModel, update, view};
use icedtea_settings::ipc;
use icedtea_settings::outputs::pump::OutputsPump;
use icedtea_ui::app::{ThemeEnv, load_layered_stylesheet};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::parse::{Stylesheet, parse_stylesheet_with_base};
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox};
use icedtea_ui::window::{Role, SurfaceSpec, Window};

const APP_ID: &str = "org.icedtea.Settings";
const TITLE: &str = "icedtea Settings";
const SIZE: (u32, u32) = (480, 420);
const STYLE: &str = include_str!("../style.css");

/// Parse `path` as a complete base theme, falling back to the bundled light
/// Adwaita sheet (with a warning) when it cannot be read — the exact
/// convention `ui/src/bin/window-probe.rs:41` uses for the same variable.
fn read_theme_file(path: &Path) -> Stylesheet {
    match std::fs::read_to_string(path) {
        Ok(css) => parse_stylesheet_with_base(&css, path.parent()),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "cannot read $ICEDTEA_UI_THEME; using bundled Adwaita");
            parse_stylesheet_with_base(icedtea_ui::BUNDLED_ADWAITA_LIGHT, None)
        }
    }
}

/// The theme stack this run composes, with this app's own sheet layered on
/// top.
///
/// `$ICEDTEA_UI_THEME` (P2-D7, spelled per the M5 contract's E3 ruling —
/// binding on both P2-D7 and P3-D6) names a path to a *complete* base theme
/// file, used whole and compiled under the default (light, no contrast
/// preference) media environment — again `window-probe.rs`'s convention, not
/// `gallery --theme`'s media-aware one, since the three bundled sheets a gate
/// writes to that path already bake in their own colours unconditionally.
/// Unset, this falls back to the real desktop theme resolution
/// (`$GTK_THEME` plus the user's own `gtk-4.0/gtk.css` override) production
/// runs use.
fn sheet() -> CompiledSheet {
    if let Ok(path) = std::env::var("ICEDTEA_UI_THEME") {
        let mut stylesheet = read_theme_file(Path::new(&path));
        stylesheet.append_layer(parse_stylesheet_with_base(STYLE, None));
        return CompiledSheet::from_stylesheet(stylesheet);
    }
    let env = ThemeEnv::from_env();
    let mut stylesheet = load_layered_stylesheet(&env);
    stylesheet.append_layer(parse_stylesheet_with_base(STYLE, None));
    CompiledSheet::compile_with_env(&stylesheet, &env.media_env())
}

/// The page the window opens on (P2-D8).
///
/// A debug/test affordance with the same role as `gallery --widget`: it lets
/// a rest-state gate photograph one page without synthesising a switcher
/// click. An unknown name falls back to the first page rather than failing
/// to start.
fn initial_page() -> icedtea_settings::pages::PageId {
    let Ok(name) = std::env::var("ICEDTEA_SETTINGS_PAGE") else {
        return icedtea_settings::pages::PageId::Appearance;
    };
    icedtea_settings::pages::PageId::ALL
        .into_iter()
        .find(|page| page.name() == name)
        .unwrap_or(icedtea_settings::pages::PageId::Appearance)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let spec = SurfaceSpec {
        role: Role::Toplevel,
        size: SIZE,
        title: TITLE.to_string(),
        app_id: APP_ID.to_string(),
    };
    let mut window = match Window::open(spec, sheet(), FontDatabase::new()) {
        Ok(window) => window,
        Err(err) => {
            tracing::error!(?err, "cannot open the settings window");
            std::process::exit(1);
        }
    };

    let (inbox, tx) = match Inbox::new() {
        Ok(pair) => pair,
        Err(err) => {
            tracing::error!(%err, "cannot create the inbox");
            std::process::exit(1);
        }
    };
    let seed_tx = tx.clone();
    let workers = ipc::spawn(tx);

    // A missing output-management global is not fatal: the Displays page
    // shows its unavailable state and everything else works.
    let pump = OutputsPump::attach(&mut window).unwrap_or_else(|err| {
        tracing::warn!(%err, "no outputs pump");
        None
    });
    let watch = pump.as_ref().map(OutputsPump::watch);

    // `OutputsConnection::from_connection` roundtrips twice inside `attach`,
    // so the initial enumeration — a `HeadsChanged`, or `ManagerUnavailable` —
    // is already queued before the loop exists. `App::on_fd` only ever fires on
    // an `InputEvent::FdReady`, and after those roundtrips the socket is quiet,
    // so nothing would wake it: seed the inbox with that first drain instead of
    // stranding it. The inbox is an fd of its own, so the messages are folded
    // on the loop's first pass.
    if let Some(pump) = pump.as_ref() {
        for msg in pump.drain() {
            if seed_tx.send(msg).is_err() {
                tracing::warn!("the inbox is gone; the initial outputs enumeration was dropped");
                break;
            }
        }
    }

    let mut model =
        SettingsModel::new(default_db_path(), workers.handles()).with_outputs(pump.clone());
    model.page = initial_page();
    let mut app = App::new(model, update, view).with_inbox(inbox);
    if let (Some(id), Some(pump)) = (watch, pump) {
        app = app.on_fd(id, move || pump.drain());
    }
    let outcome = app.run(window);
    // Apply is asynchronous: closing the window immediately after clicking it
    // used to exit while the reload worker was still inside its `redb` write,
    // and the edit was lost with no message at all. This waits, bounded, for
    // whatever is in flight.
    workers.shutdown();
    if let Err(err) = outcome {
        tracing::error!(?err, "the settings loop stopped");
        std::process::exit(1);
    }
}
