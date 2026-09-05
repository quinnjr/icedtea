//! `icedtea-settings` — a config editor for icedtea, built on `icedtea-ui`.
//!
//! One `App<SettingsModel, Msg>` on one `Role::Toplevel` window. Two external
//! event sources reach the loop through the toolkit's ingress: the reload
//! worker posts on the inbox, and the Displays page's second Wayland
//! connection is registered with `Window::watch_fd` and mapped to messages by
//! `App::on_fd`.

use icedtea_config::default_db_path;
use icedtea_settings::app::{SettingsModel, update, view};
use icedtea_settings::outputs::pump::OutputsPump;
use icedtea_settings::{ipc, probe};
use icedtea_ui::app::{ThemeEnv, load_layered_stylesheet};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::parse::parse_stylesheet_with_base;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox};
use icedtea_ui::window::{Role, SurfaceSpec, Window};

const APP_ID: &str = "org.icedtea.Settings";
const TITLE: &str = "icedtea Settings";
const SIZE: (u32, u32) = (480, 420);
const STYLE: &str = include_str!("../style.css");

/// The GTK theme stack with this app's own sheet layered on top, compiled
/// under the media environment the theme implies.
fn sheet() -> CompiledSheet {
    let env = ThemeEnv::from_env();
    let mut stylesheet = load_layered_stylesheet(&env);
    stylesheet.append_layer(parse_stylesheet_with_base(STYLE, None));
    CompiledSheet::compile_with_env(&stylesheet, &env.media_env())
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

    let model = SettingsModel::new(default_db_path(), workers).with_outputs(pump.clone());
    let mut app = App::new(model, update, view).with_inbox(inbox);
    if let (Some(id), Some(pump)) = (watch, pump) {
        app = app.on_fd(id, move || pump.drain());
    }
    if let Some(path) = probe::report_path() {
        app = app.with_probe_report(path);
    }
    if let Err(err) = app.run(window) {
        tracing::error!(?err, "the settings loop stopped");
        std::process::exit(1);
    }
}
