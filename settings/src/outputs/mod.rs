//! `zwlr_output_management_v1` client for the Displays page.
//!
//! Two layers:
//!
//! * [`protocol`] — a GTK-free core owning a *second* `wayland-client`
//!   connection and the full `Dispatch` tree for the output-management
//!   protocol. Plain-data [`Head`]/[`Mode`] snapshots out, [`HeadEdit`]
//!   configurations in. Drivable with explicit roundtrips, so the whole
//!   enumerate/apply cycle is testable against `icedtea-harness` with no GTK,
//!   gio, or glib in sight.
//! * [`client`] — the [`OutputsClient`] handle the GTK page holds. It layers a
//!   `gio::Socket` glib source over the core so the queue is pumped from GTK's
//!   main loop without a dispatch thread and without ever blocking it.

pub mod client;
pub mod protocol;

pub use client::OutputsClient;
pub use protocol::{
    Head, HeadEdit, Mode, ModeRequest, OutputsConnection, OutputsError, OutputsMsg,
};
