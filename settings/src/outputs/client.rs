//! glib main-loop integration for the [`OutputsConnection`].
//!
//! The Displays page runs inside GTK's `glib::MainContext`; it cannot block on
//! the output-management socket. So instead of a dispatch thread we hand the
//! queue's readiness fd to glib: `dup` it, wrap the copy in a [`gio::Socket`],
//! and attach a source that wakes on `IN`. The callback drains the queue
//! non-blockingly. All `unsafe` stays inside `rustix`/`gio`/`glib`.

use std::cell::{Cell, RefCell};
use std::os::fd::{AsFd, OwnedFd};
use std::rc::Rc;

use async_channel::Sender;
use glib::ControlFlow;

use super::protocol::{Head, HeadEdit, OutputsConnection, OutputsError, OutputsMsg};

/// A handle the Displays page holds: the live connection plus the glib source
/// keeping it pumped. Dropping it detaches the source.
pub struct OutputsClient {
    conn: Rc<RefCell<OutputsConnection>>,
    source_id: Option<glib::SourceId>,
    /// Set by the source callback when it returns `Break` (connection lost),
    /// so `Drop` doesn't try to remove an already-detached source.
    detached: Rc<Cell<bool>>,
}

impl OutputsClient {
    /// Connect via `WAYLAND_DISPLAY`, enumerate the current heads, and attach a
    /// dispatch source to `main_context`. Head changes and apply results are
    /// delivered over `tx`.
    ///
    /// Returns an error only if the second wayland connection cannot be opened;
    /// a *missing* manager global is not an error — it surfaces as
    /// [`OutputsMsg::ManagerUnavailable`] and [`Self::manager_present`] is
    /// `false`.
    pub fn spawn(
        main_context: &glib::MainContext,
        tx: Sender<OutputsMsg>,
    ) -> Result<OutputsClient, wayland_client::ConnectError> {
        let conn = OutputsConnection::connect_to_env(tx)?;
        Ok(Self::attach(main_context, conn))
    }

    /// Attach an already-built connection to a glib main context. Split out so
    /// tests can build a connection against a private harness socket and still
    /// exercise the glib source if they want to.
    pub fn attach(main_context: &glib::MainContext, conn: OutputsConnection) -> OutputsClient {
        let conn = Rc::new(RefCell::new(conn));
        let detached = Rc::new(Cell::new(false));

        // Duplicate the queue's readiness fd so glib owns its own copy; the
        // original stays with the EventQueue. `rustix::io::dup` keeps the one
        // unsafe syscall inside rustix. Both this dup and the `gio::Socket`
        // wrap can fail under fd exhaustion — degrade to the same
        // "output management unavailable" state the page shows when the manager
        // global is missing, rather than panicking the whole GTK app. Without a
        // source the queue is never pumped, so emit `Disconnected` (which the
        // page treats as unavailable) and return an inert, already-detached
        // handle.
        let socket = {
            let c = conn.borrow();
            rustix::io::dup(c.queue().as_fd())
                .map_err(|e| e.to_string())
                .and_then(|owned: OwnedFd| gio::Socket::from_fd(owned).map_err(|e| e.to_string()))
        };
        let socket = match socket {
            Ok(socket) => socket,
            Err(err) => {
                tracing::warn!(%err, "could not hook the outputs queue into glib; treating output management as unavailable");
                conn.borrow().notify_disconnected();
                detached.set(true);
                return OutputsClient { conn, source_id: None, detached };
            }
        };

        let cb_conn = conn.clone();
        let cb_detached = detached.clone();
        let source = gio::prelude::SocketExtManual::create_source(
            &socket,
            glib::IOCondition::IN,
            None::<&gio::Cancellable>,
            Some("wl-outputs"),
            glib::Priority::DEFAULT,
            move |_socket, _cond| {
                let mut c = cb_conn.borrow_mut();
                // Drain what is already buffered; only touch the socket if that
                // came up empty, and never block.
                let result = match c.dispatch_pending() {
                    Ok(0) => c.read_and_dispatch(),
                    other => other,
                };
                match result {
                    Ok(_) => ControlFlow::Continue,
                    Err(err) => {
                        // A dispatch/read error means the connection is gone
                        // (compositor exited, socket broke). Detaching the
                        // source is the only sane move — otherwise glib wakes
                        // us on the dead fd forever and we warn-spam. Tell the
                        // page so it drops into its unavailable state.
                        tracing::warn!(%err, "output-management connection lost; detaching source");
                        c.notify_disconnected();
                        cb_detached.set(true);
                        ControlFlow::Break
                    }
                }
            },
        );
        let source_id = source.attach(Some(main_context));

        OutputsClient {
            conn,
            source_id: Some(source_id),
            detached,
        }
    }

    /// A fresh snapshot of the current heads.
    pub fn heads(&self) -> Vec<Head> {
        self.conn.borrow().heads()
    }

    /// Whether the compositor advertised the manager global.
    pub fn manager_present(&self) -> bool {
        self.conn.borrow().manager_present()
    }

    /// Build a configuration from `edits` and apply it. Results arrive on the
    /// channel as [`OutputsMsg::ApplySucceeded`] / `ApplyFailed`.
    pub fn build_and_send_configuration(&self, edits: &[HeadEdit]) -> Result<(), OutputsError> {
        self.conn.borrow_mut().build_and_send_configuration(edits)
    }

    /// Build a configuration from `edits` and `test()` it without applying.
    pub fn test_configuration(&self, edits: &[HeadEdit]) -> Result<(), OutputsError> {
        self.conn.borrow_mut().test_configuration(edits)
    }
}

impl Drop for OutputsClient {
    fn drop(&mut self) {
        // If the callback already returned `Break`, glib has destroyed the
        // source; removing it again would log a GLib-CRITICAL. `SourceId` does
        // not auto-remove on drop, so simply letting it fall out of scope is
        // the right no-op in that case.
        if let Some(id) = self.source_id.take()
            && !self.detached.get()
        {
            id.remove();
        }
    }
}
