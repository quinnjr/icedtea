//! The `org.icedtea.WM` client. A worker thread receives the compositor's
//! signals (seeded by a `GetState` snapshot) and pushes [`WmUpdate`]s to the
//! GTK thread; [`WmProxy`] issues commands (focus/close/workspace) with a
//! separate blocking connection.

use async_channel::Sender;
use futures_util::StreamExt as _;
use icedtea_contract::{
    Snapshot, WindowInfo, WindowUpdate, WorkspaceInfo, WM_BUS_NAME, WM_PATH,
};

use crate::taskbar::WmUpdate;

const WM_IFACE: &str = "org.icedtea.WM";

/// Spawn the signal worker. It seeds with `GetState`, then forwards every
/// `org.icedtea.WM` signal as a [`WmUpdate`] until the bus drops.
pub fn spawn(tx: Sender<WmUpdate>) {
    std::thread::spawn(move || {
        zbus::block_on(async move {
            if let Err(err) = run(tx).await {
                tracing::error!(%err, "org.icedtea.WM client stopped");
            }
        });
    });
}

async fn run(tx: Sender<WmUpdate>) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, WM_BUS_NAME, WM_PATH, WM_IFACE).await?;

    // Seed from the current snapshot before watching deltas.
    let snapshot: Snapshot = proxy.call("GetState", &()).await?;
    let _ = tx.send(WmUpdate::Snapshot(snapshot)).await;

    // One stream for every org.icedtea.WM signal, dispatched by member name.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(WM_IFACE)?
        .path(WM_PATH)?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;

    while let Some(Ok(msg)) = stream.next().await {
        let member = msg.header().member().map(|m| m.as_str().to_string());
        let body = msg.body();
        let update = match member.as_deref() {
            Some("WindowOpened") => {
                body.deserialize::<(u64, WindowInfo)>().ok().map(|(_, w)| WmUpdate::Opened(w))
            }
            Some("WindowClosed") => {
                body.deserialize::<(u64, u32)>().ok().map(|(_, id)| WmUpdate::Closed(id))
            }
            Some("WindowUpdated") => body
                .deserialize::<(u64, u32, WindowUpdate)>()
                .ok()
                .map(|(_, id, update)| WmUpdate::Updated { id, update }),
            Some("WorkspaceSet") => body
                .deserialize::<(u64, u32, bool)>()
                .ok()
                .map(|(_, id, active)| WmUpdate::WorkspaceSet { id, active }),
            Some("WorkspaceList") => body
                .deserialize::<(u64, Vec<WorkspaceInfo>)>()
                .ok()
                .map(|(_, ws)| WmUpdate::WorkspaceList(ws)),
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
pub trait WmCommands {
    fn focus_window(&self, id: u32);
    fn close_window(&self, id: u32);
    fn set_workspace(&self, id: u32);
}

/// Issues `org.icedtea.WM` commands from the GTK thread. Method calls are
/// no-reply and sub-millisecond, so a blocking connection here is fine.
pub struct WmProxy {
    conn: zbus::blocking::Connection,
}

impl WmProxy {
    pub fn new() -> zbus::Result<Self> {
        Ok(WmProxy { conn: zbus::blocking::Connection::session()? })
    }
}

impl WmCommands for WmProxy {
    fn focus_window(&self, id: u32) {
        let _ = self.conn.call_method(Some(WM_BUS_NAME), WM_PATH, Some(WM_IFACE), "focus_window", &(id,));
    }
    fn close_window(&self, id: u32) {
        let _ = self.conn.call_method(Some(WM_BUS_NAME), WM_PATH, Some(WM_IFACE), "close_window", &(id,));
    }
    fn set_workspace(&self, id: u32) {
        let _ = self.conn.call_method(Some(WM_BUS_NAME), WM_PATH, Some(WM_IFACE), "set_workspace", &(id,));
    }
}
