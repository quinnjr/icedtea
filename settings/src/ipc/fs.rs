//! The filesystem worker: `stat(2)` and `redb` reads, off the loop thread.
//!
//! `update` never blocks (spec D8), and two of its arms did: `Msg::
//! WallpaperEdited` ran `std::fs::metadata` on *every keystroke* — a
//! network mount or a spun-down disk stalls the whole window inside the
//! fold — and `Msg::Revert` opened the `redb` store from inside `update`
//! itself. Both now leave through `Cmd::Task` and come back as a `Msg` on
//! the inbox, the same shape the reload and portal workers already have.

use std::path::PathBuf;

use icedtea_ui::view::InboxSender;

use crate::app::Msg;

/// What the loop asks the filesystem worker for.
pub enum FsRequest {
    /// Does this typed-or-portal-supplied path name an image this app can
    /// use? Answered with [`Msg::WallpaperValidated`].
    ValidateWallpaper { text: String },
    /// Read `db_path` back, for Revert. Answered with [`Msg::ConfigLoaded`].
    LoadConfig { db_path: PathBuf },
    /// Stop once everything already queued is served.
    Shutdown,
}

/// One worker thread serving `rx` until every sender is dropped.
///
/// Degrades rather than panics under thread exhaustion, exactly as the reload
/// and portal workers do (P2-D20): with no worker the window still runs, and
/// [`crate::ipc::WorkerHandles::fs_available`] is what the model shows the
/// user instead of a control that silently does nothing.
pub fn spawn(
    rx: crossbeam_channel::Receiver<FsRequest>,
    tx: InboxSender<Msg>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("settings-fs".to_string())
        .spawn(move || {
            while let Ok(request) = rx.recv() {
                let msg = match request {
                    FsRequest::Shutdown => return,
                    FsRequest::ValidateWallpaper { text } => Msg::WallpaperValidated {
                        text: text.clone(),
                        result: crate::pages::appearance::validate_wallpaper(&text)
                            .map(|path| path.display().to_string()),
                    },
                    FsRequest::LoadConfig { db_path } => Msg::ConfigLoaded(
                        icedtea_config::load_reportable(&db_path).map(std::sync::Arc::new),
                    ),
                };
                if tx.send(msg).is_err() {
                    // Every Inbox is gone: the app exited.
                    return;
                }
            }
        })
        .map_err(|err| tracing::warn!(%err, "no settings filesystem worker thread"))
        .ok()
}
