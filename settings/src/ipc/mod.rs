//! Worker threads and the handles `update` reaches them through.
//!
//! Everything outbound is fire-and-forget: `update` pushes onto a channel and
//! returns, and the answer arrives as a `Msg` on the app's inbox.

pub mod reload;

/// Everything `update` needs to reach a worker. Cloneable; it lives on the
/// loop thread, so nothing here has to be `Send`.
///
/// P1 ships the reload half. Contract §2.4 also specifies
/// `portal: crossbeam_channel::Sender<PortalRequest>` and
/// `choose_wallpaper(&self, current: Option<PathBuf>)`; those land with the
/// portal worker in P2 (§2.6, and §5's table assigns "§2.6 portal worker | P2"),
/// recorded as deviation P1-D7. The field is private and `spawn`'s signature is
/// already final, so adding it is a P2-local edit.
#[derive(Clone)]
pub struct WorkerHandles {
    reload: crossbeam_channel::Sender<reload::ReloadRequest>,
    // P2: portal: crossbeam_channel::Sender<portal::PortalRequest>,
}

impl WorkerHandles {
    /// Queue a save+reload. Never blocks: the answer arrives as
    /// `Msg::Applied` on the inbox. A closed channel (the worker died) is
    /// logged once and dropped — the window stays usable.
    pub fn apply(&self, cfg: icedtea_config::Config, db_path: std::path::PathBuf) {
        if self
            .reload
            .send(reload::ReloadRequest::Apply { cfg, db_path })
            .is_err()
        {
            tracing::warn!("the reload worker is gone; Apply was dropped");
        }
    }
}

/// Handles wired to a receiver the caller keeps, with no worker thread behind
/// them — what a `SettingsModel` unit test constructs (deviation P1-D8).
///
/// P2 extends the returned tuple with the portal receiver, in this same shape.
#[must_use]
pub fn handles_for_test() -> (
    WorkerHandles,
    crossbeam_channel::Receiver<reload::ReloadRequest>,
) {
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    (WorkerHandles { reload: reload_tx }, reload_rx)
}
