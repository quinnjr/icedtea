//! `icedtea-shell` — a layer-shell panel: a window/workspace taskbar driven by
//! `org.icedtea.Compositor`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. One `App` on one surface, one loop thread; D-Bus
//! runs on its own workers and reaches the loop through the inbox.

use std::rc::Rc;

use icedtea_shell::clip_client::{ClipCommands, ClipProxy};
use icedtea_shell::compositor_client::{CompositorCommands, CompositorProxy};
use icedtea_shell::panel::{self, BAR_HEIGHT, Offline, PanelModel};
use icedtea_shell::style;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::App;
use icedtea_ui::window::{LayerSpec, Role, SurfaceSpec, Window};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = run() {
        tracing::error!(%err, "icedtea-shell exited");
        std::process::exit(1);
    }
}

/// The panel's surface: anchored left/right/top, `Layer::Top`, no keyboard.
///
/// Every value is the layer-shell call it replaces. Anchored **top**, not
/// bottom: the source comment stands — a bottom bar lands below the visible
/// area on a display whose viewport is shorter than the reported output (a VM
/// console). `keyboard: None` matches GTK4 layer-shell's unset default; the
/// panel takes no keyboard focus, and the popover's search field has no IME
/// until M6. `exclusive_zone` is the literal `BAR_HEIGHT` because `LayerSpec`
/// has no "auto" (contract §6 P5-D6). The initial size is `(800, 28)`, the
/// pair `set_default_size(800, 28)` + `set_size_request(-1, 28)` forced: a
/// 0-height layer surface never commits a real buffer.
fn spec() -> SurfaceSpec {
    SurfaceSpec {
        role: Role::Layer(LayerSpec {
            layer: zwlr_layer_shell_v1::Layer::Top,
            anchor: zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right
                | zwlr_layer_surface_v1::Anchor::Top,
            margin: [0, 0, 0, 0],
            exclusive_zone: BAR_HEIGHT,
            keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::None,
        }),
        #[allow(
            clippy::cast_sign_loss,
            reason = "BAR_HEIGHT is a positive literal constant"
        )]
        size: (800, BAR_HEIGHT as u32),
        // A layer surface has no `namespace` field: `title` is the namespace.
        title: "icedtea-shell".to_string(),
        app_id: "org.icedtea.Shell".to_string(),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let wm: Rc<dyn CompositorCommands> = match CompositorProxy::new() {
        Ok(proxy) => Rc::new(proxy),
        Err(err) => {
            tracing::error!(%err, "no session bus; window commands disabled");
            Rc::new(Offline)
        }
    };
    let clip: Rc<dyn ClipCommands> = match ClipProxy::new() {
        Ok(proxy) => Rc::new(proxy),
        Err(err) => {
            tracing::error!(%err, "no session bus; clipboard commands disabled");
            Rc::new(Offline)
        }
    };

    let window = Window::open(spec(), style::sheet(), FontDatabase::new())?;
    App::new(PanelModel::new(wm, clip), panel::update, panel::view).run(window)?;
    Ok(())
}
