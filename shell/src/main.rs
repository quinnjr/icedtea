//! `icedtea-shell` — a layer-shell panel: a window/workspace taskbar driven by
//! `org.icedtea.Compositor`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. One `App` on one surface, one loop thread; D-Bus
//! runs on its own workers and reaches the loop through the inbox.

use std::rc::Rc;
use std::sync::Arc;

use icedtea_shell::clip_client::{self, ClipCommands, ClipProxy};
use icedtea_shell::clipboard::ClipUpdate;
use icedtea_shell::compositor_client::{self, CompositorCommands, CompositorProxy};
use icedtea_shell::panel::{self, Msg, Offline, PanelModel};
use icedtea_shell::style;
use icedtea_shell::taskbar::CompositorUpdate;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox, InboxSender, PopupEvent};
use icedtea_ui::window::Window;

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

/// Drain `rx` on its own thread, wrapping each value into a `Msg` for the
/// app's inbox.
///
/// The replacement for the GLib `spawn_future_local` future `bridge.rs` used:
/// there is no GLib main loop to post onto, and the `App`'s loop is a hand-rolled poll
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
        loop {
            match rx.recv_blocking() {
                Ok(value) => {
                    if tx.send(wrap(value)).is_err() {
                        // The app dropped its `Inbox`: the expected shutdown
                        // path, so this stays quiet.
                        break;
                    }
                }
                Err(_) => {
                    // The D-Bus worker's sender dropped — the worker thread
                    // died. For `clip_client` this is non-fatal, so the panel
                    // is otherwise left with a permanently stale pipeline and
                    // no trace; log it so a dead worker is visible.
                    tracing::warn!(
                        "a D-Bus worker's channel closed; its updates have stopped reaching the panel"
                    );
                    break;
                }
            }
        }
    })
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let window = Window::open(panel::spec(), style::sheet(), FontDatabase::new())?;
    let (inbox, tx) = Inbox::<Msg>::new()?;
    let width_tx = tx.clone();

    // Each client's proxy, forward thread and worker are created together: when
    // `*Proxy::new()` fails (no session bus) the arm installs `Offline` and
    // starts no worker at all. Spawning a worker anyway would run a
    // `Connection::session()` that fails the very same way — and the compositor
    // worker's `process::exit(1)` (finding F7) would then crash-loop the whole
    // panel under systemd `Restart=always`, taking the working clipboard half
    // down with it, instead of degrading gracefully behind `Offline`.
    let wm: Rc<dyn CompositorCommands> = match CompositorProxy::new() {
        Ok(proxy) => {
            let (comp_tx, comp_rx) = async_channel::unbounded::<CompositorUpdate>();
            let _comp_forward = forward(comp_rx, tx.clone(), |u| Msg::Compositor(Arc::new(u)));
            compositor_client::spawn(comp_tx);
            Rc::new(proxy)
        }
        Err(err) => {
            tracing::error!(%err, "no session bus; window commands disabled");
            Rc::new(Offline)
        }
    };
    let clip: Rc<dyn ClipCommands> = match ClipProxy::new() {
        Ok(proxy) => {
            let (clip_tx, clip_rx) = async_channel::unbounded::<ClipUpdate>();
            let _clip_forward = forward(clip_rx, tx.clone(), |u| Msg::Clip(Arc::new(u)));
            clip_client::spawn(clip_tx);
            Rc::new(proxy)
        }
        Err(err) => {
            tracing::error!(%err, "no session bus; clipboard commands disabled");
            Rc::new(Offline)
        }
    };

    // P5-D9: `update` has no `&Window`, so the `clip` button's box is published
    // once per frame into the cell the model shares with it — the popover's
    // anchor. The whole frame hook is `panel::frame_hook`, the one factory the
    // integration harness calls too (M5 finding #3), so there is a single
    // source of truth for what a frame publishes.
    let model = PanelModel::new(wm, clip);
    let clip_rect = model.clip_rect.clone();
    let open_popover = model.open_popover_cell.clone();
    let bar_width = model.bar_width;
    // The open popover's own probe lines, written beside `$ICEDTEA_PROBE_REPORT`
    // (App::run owns that append-only file for the window's own lines). Only a
    // harness sets the env var, so this is inert in a real session.
    let popup_report = std::env::var_os("ICEDTEA_PROBE_REPORT").map(|value| {
        let mut path = std::path::PathBuf::from(value);
        path.set_extension("popups");
        path
    });

    App::new(model, panel::update, panel::view)
        .with_inbox(inbox)
        .on_popup(|ev| match ev {
            PopupEvent::Opened(key) => Some(Msg::PopoverOpened(key)),
            PopupEvent::Dismissed(key) => Some(Msg::PopoverDismissed(key)),
            // `PopupEvent` is `#[non_exhaustive]`; a future variant this
            // shell does not yet know about is simply not turned into a
            // message.
            _ => None,
        })
        .on_frame(panel::frame_hook(
            clip_rect,
            open_popover,
            popup_report,
            width_tx,
            bar_width,
        ))
        .run(window)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use icedtea_shell::panel::Msg;
    use icedtea_ui::layout::{Allocation, Rect};
    use icedtea_ui::view::Inbox;

    fn alloc(x: f32, y: f32, w: f32, h: f32) -> Allocation {
        Allocation {
            border_box: Rect::new(x, y, w, h),
            content_box: Rect::new(x, y, w, h),
            border: [0.0; 4],
            padding: [0.0; 4],
        }
    }

    /// `frame_hook`'s clip-rect publication in isolation from `Window`: the
    /// popover's anchor is the `clip` button's border box, when the window
    /// can resolve it. (`clip_border_box` now lives in the lib alongside
    /// `frame_hook`, its sole non-test caller.)
    #[test]
    fn the_clip_box_is_published_when_the_button_resolves() {
        let got = icedtea_shell::panel::clip_border_box(|id| match id {
            "clip" => Some(alloc(700.0, 0.0, 40.0, 28.0)),
            _ => None,
        });
        assert_eq!(got, Some(Rect::new(700.0, 0.0, 40.0, 28.0)));
    }

    /// Before the first layout (or if the id ever goes missing), the anchor
    /// is `None` rather than a stale box: `update` falls back to a sane
    /// default when it is.
    #[test]
    fn the_clip_box_is_none_when_the_button_has_not_resolved() {
        let got = icedtea_shell::panel::clip_border_box(|_| None);
        assert_eq!(got, None);
    }

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
