//! The `org.icedtea.Compositor` client. A worker thread receives the compositor's
//! signals (seeded by a `GetState` snapshot) and pushes [`CompositorUpdate`]s to the
//! GTK thread; [`CompositorProxy`] issues commands (focus/close/workspace) with a
//! separate blocking connection.

use async_channel::Sender;
use futures_util::StreamExt as _;
use icedtea_contract::{
    Snapshot, WindowInfo, WindowUpdate, WorkspaceInfo, COMPOSITOR_BUS_NAME, COMPOSITOR_PATH,
};

use crate::taskbar::CompositorUpdate;

const COMPOSITOR_IFACE: &str = "org.icedtea.Compositor";

/// Spawn the signal worker. It seeds with `GetState`, then forwards every
/// `org.icedtea.Compositor` signal as a [`CompositorUpdate`] until the bus drops.
pub fn spawn(tx: Sender<CompositorUpdate>) {
    std::thread::spawn(move || {
        zbus::block_on(async move {
            if let Err(err) = run(tx).await {
                tracing::error!(%err, "org.icedtea.Compositor client stopped");
            }
        });
    });
}

async fn run(tx: Sender<CompositorUpdate>) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, COMPOSITOR_BUS_NAME, COMPOSITOR_PATH, COMPOSITOR_IFACE).await?;

    // Seed from the current snapshot before watching deltas.
    let snapshot: Snapshot = proxy.call("GetState", &()).await?;
    let _ = tx.send(CompositorUpdate::Snapshot(snapshot)).await;

    // One stream for every org.icedtea.Compositor signal, dispatched by member name.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(COMPOSITOR_IFACE)?
        .path(COMPOSITOR_PATH)?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;

    while let Some(Ok(msg)) = stream.next().await {
        let member = msg.header().member().map(|m| m.as_str().to_string());
        let body = msg.body();
        let update = match member.as_deref() {
            Some("WindowOpened") => {
                body.deserialize::<(u64, WindowInfo)>().ok().map(|(_, w)| CompositorUpdate::Opened(w))
            }
            Some("WindowClosed") => {
                body.deserialize::<(u64, u32)>().ok().map(|(_, id)| CompositorUpdate::Closed(id))
            }
            Some("WindowUpdated") => body
                .deserialize::<(u64, u32, WindowUpdate)>()
                .ok()
                .map(|(_, id, update)| CompositorUpdate::Updated { id, update }),
            Some("WorkspaceSet") => body
                .deserialize::<(u64, u32, bool)>()
                .ok()
                .map(|(_, id, active)| CompositorUpdate::WorkspaceSet { id, active }),
            Some("WorkspaceList") => body
                .deserialize::<(u64, Vec<WorkspaceInfo>)>()
                .ok()
                .map(|(_, ws)| CompositorUpdate::WorkspaceList(ws)),
            _ => None,
        };
        if let Some(update) = update
            && tx.send(update).await.is_err()
        {
            break; // GTK side gone.
        }
    }
    Ok(())
}

/// The taskbar's command surface — abstracted so a test can inject a recording
/// mock in place of the real D-Bus proxy.
pub trait CompositorCommands {
    fn focus_window(&self, id: u32);
    fn close_window(&self, id: u32);
    fn set_workspace(&self, id: u32);
}

/// Issues `org.icedtea.Compositor` commands from the GTK thread. Method calls are
/// no-reply and sub-millisecond, so a blocking connection here is fine.
pub struct CompositorProxy {
    conn: zbus::blocking::Connection,
}

impl CompositorProxy {
    pub fn new() -> zbus::Result<Self> {
        Ok(CompositorProxy { conn: zbus::blocking::Connection::session()? })
    }
}

impl CompositorCommands for CompositorProxy {
    // zbus's #[interface] exposes Rust methods in PascalCase, so the wire
    // members are FocusWindow/CloseWindow/SetWorkspace (matching GetState).
    fn focus_window(&self, id: u32) {
        let _ = self.conn.call_method(Some(COMPOSITOR_BUS_NAME), COMPOSITOR_PATH, Some(COMPOSITOR_IFACE), "FocusWindow", &(id,));
    }
    fn close_window(&self, id: u32) {
        let _ = self.conn.call_method(Some(COMPOSITOR_BUS_NAME), COMPOSITOR_PATH, Some(COMPOSITOR_IFACE), "CloseWindow", &(id,));
    }
    fn set_workspace(&self, id: u32) {
        let _ = self.conn.call_method(Some(COMPOSITOR_BUS_NAME), COMPOSITOR_PATH, Some(COMPOSITOR_IFACE), "SetWorkspace", &(id,));
    }
}
