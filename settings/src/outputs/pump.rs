//! The Displays page's second `wayland-client` connection, pumped by the
//! toolkit's poll loop instead of a glib source.
//!
//! `Window::watch_fd` takes a dup of the `EventQueue`'s readiness fd (M5-D1),
//! and `App::on_fd` calls [`OutputsPump::drain`] whenever it fires. Nothing
//! here blocks: `drain` dispatches what is already buffered and reads only if
//! that dispatched nothing, exactly as `protocol.rs:367-379` does.

use std::cell::RefCell;
use std::rc::Rc;

use icedtea_ui::window::{Interest, WatchId, Window};

use crate::app::Msg;
use crate::outputs::{HeadEdit, OutputsConnection, OutputsMsg};

/// Owns the second connection and its watch.
#[derive(Clone)]
pub struct OutputsPump {
    conn: Rc<RefCell<OutputsConnection>>,
    rx: async_channel::Receiver<OutputsMsg>,
    watch: WatchId,
}

impl OutputsPump {
    /// Connect, register the queue fd with `window`, and return the pump.
    ///
    /// A connect failure — no compositor, no output-management global, fd
    /// exhaustion — is **not** fatal: it returns `Ok(None)` and the page shows
    /// its "output management unavailable" state, the exact degradation the
    /// deleted `OutputsClient` had.
    ///
    /// # Errors
    ///
    /// Never today: every failure path yields `Ok(None)`. The signature keeps
    /// the `io::Result` the contract froze so a future registration failure has
    /// somewhere to go.
    pub fn attach(window: &mut Window) -> std::io::Result<Option<OutputsPump>> {
        let (tx, rx) = async_channel::unbounded::<OutputsMsg>();
        let conn = match OutputsConnection::connect_to_env(tx) {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!(%err, "no outputs connection; the Displays page is unavailable");
                return Ok(None);
            }
        };
        // Dup so the window owns its own fd and the original stays with the
        // EventQueue — the same dup `outputs/client.rs:61` did for gio::Socket.
        let fd = match rustix::io::dup(std::os::fd::AsFd::as_fd(conn.queue())) {
            Ok(fd) => fd,
            Err(err) => {
                tracing::warn!(%err, "cannot dup the outputs queue fd; page unavailable");
                conn.notify_disconnected();
                return Ok(None);
            }
        };
        let watch = window.watch_fd(fd, Interest::Read);
        Ok(Some(OutputsPump {
            conn: Rc::new(RefCell::new(conn)),
            rx,
            watch,
        }))
    }

    /// The watch `App::on_fd` is keyed on.
    #[must_use]
    pub fn watch(&self) -> WatchId {
        self.watch
    }

    /// The `App::on_fd` body: dispatch, then fold every queued protocol
    /// message into a `Msg`. Never blocks, never `blocking_dispatch`.
    #[must_use]
    pub fn drain(&self) -> Vec<Msg> {
        {
            let mut conn = self.conn.borrow_mut();
            let dispatched = conn.dispatch_pending();
            let outcome = match dispatched {
                Ok(0) => conn.read_and_dispatch(),
                other => other,
            };
            if let Err(err) = outcome {
                tracing::warn!(%err, "the outputs connection died");
                conn.notify_disconnected();
            }
        }
        let mut msgs = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            msgs.push(Msg::Outputs(std::sync::Arc::new(msg)));
        }
        msgs
    }

    /// Preview a configuration. Fire-and-forget, from `Cmd::Task`; the reply
    /// arrives as `OutputsMsg::ApplySucceeded { is_test: true }` or its
    /// failure/cancel siblings.
    pub fn test_configuration(&self, edits: &[HeadEdit]) {
        if let Err(err) = self.conn.borrow_mut().test_configuration(edits) {
            tracing::warn!(?err, "test configuration could not be sent");
        }
        self.flush();
    }

    /// Apply a configuration. Same fire-and-forget shape as
    /// [`OutputsPump::test_configuration`].
    pub fn build_and_send_configuration(&self, edits: &[HeadEdit]) {
        if let Err(err) = self.conn.borrow_mut().build_and_send_configuration(edits) {
            tracing::warn!(?err, "configuration could not be sent");
        }
        self.flush();
    }

    /// Push queued requests out. The toolkit's loop flushes its own
    /// connection, never this one.
    pub fn flush(&self) {
        if let Err(err) = self.conn.borrow().flush() {
            tracing::warn!(%err, "flushing the outputs connection failed");
        }
    }
}
