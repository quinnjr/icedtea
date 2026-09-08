//! The Displays page's second `wayland-client` connection, pumped by the
//! toolkit's poll loop instead of a glib source.
//!
//! `Window::watch_fd` takes a dup of the `EventQueue`'s readiness fd (M5-D1),
//! and `App::on_fd` calls [`OutputsPump::drain`] whenever it fires. Nothing
//! here blocks: `drain` dispatches what is already buffered and reads only if
//! that dispatched nothing, exactly as `protocol.rs:367-379` does.

use std::cell::{Cell, RefCell};
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
    /// Latched the first time a dispatch or read errors. A dead socket stays
    /// POLLIN-ready forever — the toolkit reports HUP/ERR and never retires a
    /// foreign fd on its own (`ui/src/window/mod.rs`) — so without this a
    /// second wake would dispatch-error, warn and re-emit `Disconnected`
    /// again, and again. `update`'s `Disconnected` arm answers with
    /// `Cmd::Unwatch`; this latch is the belt to that pair of braces, and
    /// covers the window between the error and the unwatch landing.
    dead: Rc<Cell<bool>>,
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
        Self::attach_connection(window, conn, rx)
    }

    /// [`OutputsPump::attach`], against one named compositor socket.
    ///
    /// `$WAYLAND_DISPLAY` is process-global, and libtest runs a binary's tests
    /// in parallel threads of one process: a test that rewrote it to reach its
    /// own harness compositor rewrote it for every test beside it.
    /// `OutputsConnection::connect_to_path` is the way out, and this is the
    /// pump-shaped door to it.
    ///
    /// # Errors
    ///
    /// The same shape as [`OutputsPump::attach`]: a connect failure is
    /// `Ok(None)`, not an error.
    pub fn attach_at_path(
        window: &mut Window,
        path: impl AsRef<std::path::Path>,
    ) -> std::io::Result<Option<OutputsPump>> {
        let (tx, rx) = async_channel::unbounded::<OutputsMsg>();
        let conn = match OutputsConnection::connect_to_path(path, tx) {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!(%err, "no outputs connection; the Displays page is unavailable");
                return Ok(None);
            }
        };
        Self::attach_connection(window, conn, rx)
    }

    /// The half both constructors share, over a connection already made.
    fn attach_connection(
        window: &mut Window,
        conn: OutputsConnection,
        rx: async_channel::Receiver<OutputsMsg>,
    ) -> std::io::Result<Option<OutputsPump>> {
        // Dup so the window owns its own fd and the original stays with the
        // EventQueue.
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
            dead: Rc::new(Cell::new(false)),
        }))
    }

    /// The watch `App::on_fd` is keyed on.
    #[must_use]
    pub fn watch(&self) -> WatchId {
        self.watch
    }

    /// True once the output-management global was bound.
    ///
    /// `attach` roundtrips twice, so this is final by the time the pump
    /// exists: a compositor with no `zwlr_output_manager_v1` yields `false`
    /// and the model must start with the Displays page out of service rather
    /// than optimistically available, because the corrective
    /// `ManagerUnavailable` is queued *before* the loop starts and no further
    /// event will ever arrive to carry it.
    #[must_use]
    pub fn manager_present(&self) -> bool {
        self.conn.borrow().manager_present()
    }

    /// True once a dispatch or read has errored and the pump latched shut.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.dead.get()
    }

    /// The `App::on_fd` body: dispatch, then fold every queued protocol
    /// message into a `Msg`. Never blocks, never `blocking_dispatch`.
    ///
    /// Once the connection has errored the pump is latched and every later
    /// call is a no-op returning no messages — the dead fd never warn-spams
    /// and never re-emits `Disconnected`.
    #[must_use]
    pub fn drain(&self) -> Vec<Msg> {
        if self.dead.get() {
            return Vec::new();
        }
        {
            let mut conn = self.conn.borrow_mut();
            let dispatched = conn.dispatch_pending();
            let outcome = match dispatched {
                Ok(0) => conn.read_and_dispatch(),
                other => other,
            };
            if let Err(err) = outcome {
                tracing::warn!(%err, "the outputs connection died");
                self.dead.set(true);
                conn.notify_disconnected();
            }
        }
        let mut msgs = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            msgs.push(Msg::Outputs(std::sync::Arc::new(msg)));
        }
        msgs
    }

    /// Ship `edits` as a **preview** configuration.
    ///
    /// Non-blocking: it builds the configuration objects and flushes. The
    /// answer arrives later as `Msg::Outputs(ApplySucceeded/ApplyFailed)`.
    ///
    /// Returns the error when the request could not even be attempted — no
    /// manager global, an edit naming a head that is not present, or a flush
    /// failure. `update` needs that synchronously to keep the in-flight latch
    /// honest, which `Cmd::Task` (no return value) cannot deliver (plan
    /// P4-D8).
    ///
    /// # Errors
    /// [`crate::outputs::OutputsError`] from `OutputsConnection`.
    pub fn test_configuration(
        &self,
        edits: &[HeadEdit],
    ) -> Result<(), crate::outputs::OutputsError> {
        self.conn.borrow_mut().test_configuration(edits)?;
        self.flush()
    }

    /// Ship `edits` as a **real** configuration. See
    /// [`OutputsPump::test_configuration`] for the return contract.
    ///
    /// # Errors
    /// [`crate::outputs::OutputsError`] from `OutputsConnection`.
    pub fn build_and_send_configuration(
        &self,
        edits: &[HeadEdit],
    ) -> Result<(), crate::outputs::OutputsError> {
        self.conn.borrow_mut().build_and_send_configuration(edits)?;
        self.flush()
    }

    /// Push queued requests out. The toolkit's loop flushes its own
    /// connection, never this one.
    ///
    /// A flush failure that does not HUP the fd would otherwise be swallowed
    /// (warn-only) while the build path returned `Ok`, so no reply ever lands
    /// and `submit()`'s in-flight latch never clears. Instead: latch the pump
    /// `dead`, emit `Disconnected` so `update` can unwatch and tear down, and
    /// return the error so `submit()`'s `Err` arm surfaces it and leaves the
    /// busy state cleared (plan P4-D8).
    ///
    /// # Errors
    /// [`crate::outputs::OutputsError::Protocol`] wrapping the flush failure.
    pub fn flush(&self) -> Result<(), crate::outputs::OutputsError> {
        if self.dead.get() {
            return Ok(());
        }
        let conn = self.conn.borrow();
        if let Err(err) = conn.flush() {
            tracing::warn!(%err, "flushing the outputs connection failed");
            self.dead.set(true);
            conn.notify_disconnected();
            return Err(crate::outputs::OutputsError::Protocol(err));
        }
        Ok(())
    }
}
