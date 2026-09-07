//! `zwlr_output_management_v1` client for the Displays page.
//!
//! Two layers:
//!
//! * [`protocol`] — a GTK-free core owning a *second* `wayland-client`
//!   connection and the full `Dispatch` tree for the output-management
//!   protocol. Plain-data [`Head`]/[`Mode`] snapshots out, [`HeadEdit`]
//!   configurations in. Drivable with explicit roundtrips, so the whole
//!   enumerate/apply cycle is testable against `icedtea-harness` with no
//!   toolkit dependency in sight.
//! * [`pump`] — the [`pump::OutputsPump`] handle `main.rs` holds. It registers
//!   the core's queue fd with `Window::watch_fd` so the queue is pumped from
//!   the toolkit's own poll loop via `App::on_fd`, without a dispatch thread
//!   and without ever blocking it.

pub mod protocol;
pub mod pump;

pub use protocol::{
    ConfigData, Head, HeadEdit, Mode, ModeRequest, OutputsConnection, OutputsError, OutputsMsg,
};
