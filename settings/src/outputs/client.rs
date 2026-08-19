//! glib main-loop integration for the [`OutputsConnection`].
//!
//! The Displays page runs inside GTK's `glib::MainContext`; it cannot block on
//! the output-management socket. So instead of a dispatch thread we hand the
//! queue's readiness fd to glib: `dup` it, wrap the copy in a [`gio::Socket`],
//! and attach a source that wakes on `IN`. The callback drains the queue
//! non-blockingly. All `unsafe` stays inside `rustix`/`gio`/`glib`.

use std::cell::RefCell;
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
        // Duplicate the queue's readiness fd so glib owns its own copy; the
        // original stays with the EventQueue. `rustix::io::dup` keeps the one
        // unsafe syscall inside rustix.
        let owned: OwnedFd = rustix::io::dup(conn.queue().as_fd())
            .expect("dup of wayland queue fd");
        let socket = gio::Socket::from_fd(owned).expect("gio::Socket from wayland queue fd");

        let conn = Rc::new(RefCell::new(conn));
        let cb_conn = conn.clone();
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
                match c.dispatch_pending() {
                    Ok(0) => {
                        let _ = c.read_and_dispatch();
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(%err, "output-management dispatch failed");
                    }
                }
                ControlFlow::Continue
            },
        );
        let source_id = source.attach(Some(main_context));

        OutputsClient {
            conn,
            source_id: Some(source_id),
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
        if let Some(id) = self.source_id.take() {
            id.remove();
        }
    }
}
