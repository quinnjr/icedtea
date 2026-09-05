//! Worker threads and the handles `update` reaches them through.
//!
//! Everything outbound is fire-and-forget: `update` pushes onto a channel and
//! returns, and the answer arrives as a `Msg` on the app's inbox.

pub mod portal;
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

use icedtea_ui::view::InboxSender;

/// Start every worker against `tx` and return their handles.
///
/// P1 starts one: the reload worker. P2 starts the portal worker here too, from
/// the same `tx` (deviation P1-D7); this signature is contract §2.4's final one
/// and does not change when it does.
#[must_use]
pub fn spawn(tx: InboxSender<crate::app::Msg>) -> WorkerHandles {
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    // The join handle is deliberately dropped: the worker's lifetime is the
    // process's, and it stops itself when the channel or the inbox closes.
    drop(reload::spawn(reload_rx, tx));
    WorkerHandles { reload: reload_tx }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// The whole worker round trip with no compositor on the bus: the request
    /// goes out on a channel, the config lands on disk, and `Msg::Applied`
    /// comes back through the inbox — `ReloadOutcome::CompositorAbsent`
    /// unless something really is listening.
    ///
    /// Mutation check: drop the `tx.send(...)` in `reload::spawn`'s request
    /// arm; this test times out and fails. Restore.
    #[test]
    fn the_reload_worker_writes_the_config_and_answers_on_the_inbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("config.redb");
        let (inbox, tx) = icedtea_ui::view::Inbox::<crate::app::Msg>::new().expect("inbox");
        let handles = super::spawn(tx);

        let mut cfg = icedtea_config::default_config();
        cfg.appearance.palette.accent = "#ff00aa".to_string();
        handles.apply(cfg, db.clone());

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut applied = None;
        while std::time::Instant::now() < deadline && applied.is_none() {
            applied = inbox.try_recv();
            std::thread::sleep(Duration::from_millis(20));
        }
        match applied {
            Some(crate::app::Msg::Applied(Ok(_))) => {}
            other => panic!("expected Msg::Applied(Ok(..)), got {other:?}"),
        }
        let on_disk = icedtea_config::load_or_default(&db);
        assert_eq!(on_disk.appearance.palette.accent, "#ff00aa");
    }
}
