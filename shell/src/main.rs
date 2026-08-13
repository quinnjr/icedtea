//! `icedtea-shell` — a GTK4 layer-shell panel: a window/workspace taskbar
//! driven by `org.icedtea.WM`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. All widgets live on the GTK main thread; D-Bus runs
//! on a worker, bridged by a glib channel.

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, CssProvider, Orientation};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

const APP_ID: &str = "org.icedtea.Shell";

fn main() {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_panel);
    app.run();
}

fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_data(include_str!("../style.css"));
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_panel(app: &Application) {
    let window = ApplicationWindow::new(app);

    // Layer-shell: a bottom bar reserving its own height.
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.auto_exclusive_zone_enable();
    for edge in [Edge::Left, Edge::Right, Edge::Bottom] {
        window.set_anchor(edge, true);
    }

    let bar = GtkBox::new(Orientation::Horizontal, 6);
    bar.set_widget_name("bar");
    window.set_child(Some(&bar));
    window.present();
}
