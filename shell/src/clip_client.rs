//! The `org.icedtea.Clipboard` client: a worker re-fetches `get_history` on each
//! `history_changed` and pushes it to the GTK thread; [`ClipProxy`] issues
//! activate/pin/remove/clear.

use async_channel::Sender;
use futures_util::StreamExt as _;
use icedtea_contract::{ClipEntry, CLIP_BUS_NAME, CLIP_PATH};

use crate::clipboard::ClipUpdate;

const CLIP_IFACE: &str = "org.icedtea.Clipboard";

/// The clipboard command surface — abstracted so a test can inject a mock.
pub trait ClipCommands {
    fn activate(&self, id: u64);
    fn pin(&self, id: u64, on: bool);
    fn remove(&self, id: u64);
    fn clear(&self);
}

/// Spawn the history worker: seed with `get_history`, then re-fetch on each
/// `history_changed`.
pub fn spawn(tx: Sender<ClipUpdate>) {
    std::thread::spawn(move || {
        zbus::block_on(async move {
            if let Err(err) = run(tx).await {
                tracing::error!(%err, "org.icedtea.Clipboard client stopped");
            }
        });
    });
}

async fn run(tx: Sender<ClipUpdate>) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, CLIP_BUS_NAME, CLIP_PATH, CLIP_IFACE).await?;

    let seed: Vec<ClipEntry> = proxy.call("get_history", &()).await?;
    let _ = tx.send(ClipUpdate::History(seed)).await;

    let mut changed = proxy.receive_signal("history_changed").await?;
    while (changed.next().await).is_some() {
        if let Ok(history) = proxy.call::<_, _, Vec<ClipEntry>>("get_history", &()).await
            && tx.send(ClipUpdate::History(history)).await.is_err()
        {
            break; // GTK side gone.
        }
    }
    Ok(())
}

/// Issues `org.icedtea.Clipboard` commands from the GTK thread.
pub struct ClipProxy {
    conn: zbus::blocking::Connection,
}

impl ClipProxy {
    pub fn new() -> zbus::Result<Self> {
        Ok(ClipProxy { conn: zbus::blocking::Connection::session()? })
    }
}

impl ClipCommands for ClipProxy {
    fn activate(&self, id: u64) {
        let _ = self.conn.call_method(Some(CLIP_BUS_NAME), CLIP_PATH, Some(CLIP_IFACE), "activate", &(id,));
    }
    fn pin(&self, id: u64, on: bool) {
        let _ = self.conn.call_method(Some(CLIP_BUS_NAME), CLIP_PATH, Some(CLIP_IFACE), "pin", &(id, on));
    }
    fn remove(&self, id: u64) {
        let _ = self.conn.call_method(Some(CLIP_BUS_NAME), CLIP_PATH, Some(CLIP_IFACE), "remove", &(id,));
    }
    fn clear(&self) {
        let _ = self.conn.call_method(Some(CLIP_BUS_NAME), CLIP_PATH, Some(CLIP_IFACE), "clear", &());
    }
}
