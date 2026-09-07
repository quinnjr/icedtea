//! `icedtea-shell` — a layer-shell panel: a window/workspace taskbar driven by
//! `org.icedtea.Compositor`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. One `App` on one surface, one loop thread; D-Bus
//! runs on its own workers and reaches the loop through the inbox.

use std::rc::Rc;
use std::sync::Arc;

use icedtea_shell::clip_client::{self, ClipCommands, ClipProxy};
use icedtea_shell::clipboard::ClipUpdate;
use icedtea_shell::compositor_client::{self, CompositorCommands, CompositorProxy};
use icedtea_shell::panel::{self, BAR_HEIGHT, Msg, Offline, PanelModel};
use icedtea_shell::style;
use icedtea_shell::taskbar::CompositorUpdate;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox, InboxSender, PopupEvent};
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

/// Drain `rx` on its own thread, wrapping each value into a `Msg` for the
/// app's inbox.
///
/// The replacement for `bridge.rs`'s `glib::spawn_future_local`: there is no
/// GLib main loop to post onto, and the `App`'s loop is a hand-rolled poll
/// over the Wayland fd plus whatever `Window::watch_fd` was given. The inbox
/// *is* that hook — `send` queues the message and writes one byte to the pipe
/// the loop already polls.
///
/// `recv_blocking` rather than an async runtime: this thread has exactly one
/// job, the D-Bus client already owns its own executor, and a blocking receive
/// costs nothing while idle. A failed send means the app dropped its `Inbox`,
/// which is how the thread learns to end.
fn forward<T: Send + 'static>(
    rx: async_channel::Receiver<T>,
    tx: InboxSender<Msg>,
    wrap: impl Fn(T) -> Msg + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(value) = rx.recv_blocking() {
            if tx.send(wrap(value)).is_err() {
                break;
            }
        }
    })
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
    let (inbox, tx) = Inbox::<Msg>::new()?;

    // Both clients keep their own `async_channel`, their own worker thread and
    // their own connection; only where their output lands has changed.
    let (comp_tx, comp_rx) = async_channel::unbounded::<CompositorUpdate>();
    let _comp_forward = forward(comp_rx, tx.clone(), |u| Msg::Compositor(Arc::new(u)));
    compositor_client::spawn(comp_tx);

    let (clip_tx, clip_rx) = async_channel::unbounded::<ClipUpdate>();
    let _clip_forward = forward(clip_rx, tx, |u| Msg::Clip(Arc::new(u)));
    clip_client::spawn(clip_tx);

    App::new(PanelModel::new(wm, clip), panel::update, panel::view)
        .with_inbox(inbox)
        .on_popup(|ev| match ev {
            PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
            PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
            // `PopupEvent` is `#[non_exhaustive]`; a future variant this
            // shell does not yet know about is simply not turned into a
            // message.
            _ => None,
        })
        .run(window)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use icedtea_shell::panel::Msg;
    use icedtea_ui::view::Inbox;

    /// Finding F7 is an architectural invariant, not a style preference: the
    /// shell and compositor marshal the contract types independently, so a
    /// `SignatureMismatch` on `GetState` must take the process down —
    /// `Restart=always` in `shell/systemd/icedtea-shell.service` is what then
    /// replaces the stale binary with a matching build. Logging and continuing
    /// would leave a permanently stale taskbar that systemd never restarts.
    /// A source-level assertion because the call path needs a live session bus
    /// and a running compositor.
    #[test]
    fn the_fatal_worker_path_still_exits() {
        let source = include_str!("compositor_client.rs");
        assert!(
            source.contains("std::process::exit(1)"),
            "compositor_client::spawn must still kill the process on a fatal \
             worker error (finding F7); no part may soften this into a log"
        );
    }

    /// A forward thread must not outlive the app: once the `Inbox` is dropped
    /// every `send` fails, and the thread's job is to notice and end. A leaked
    /// thread per client would keep a D-Bus connection alive after the panel
    /// closed.
    #[test]
    fn a_forward_thread_ends_when_the_app_drops_its_inbox() {
        let (inbox, tx) = Inbox::<Msg>::new().expect("inbox");
        let (value_tx, value_rx) = async_channel::unbounded::<u32>();
        let handle = super::forward(value_rx, tx, Msg::WorkspaceClicked);
        drop(inbox);
        // Enough traffic that the thread must attempt at least one send.
        for i in 0..4 {
            value_tx.send_blocking(i).expect("queue");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "the forward thread must end once the inbox is gone"
        );
        handle.join().expect("forward thread");
    }
}
