//! Worker threads and the handles `update` reaches them through.
//!
//! Everything outbound is fire-and-forget: `update` pushes onto a channel and
//! returns, and the answer arrives as a `Msg` on the app's inbox.

pub mod fs;
pub mod portal;
pub mod reload;

/// Everything `update` needs to reach a worker. Cloneable; it lives on the
/// loop thread, so nothing here has to be `Send`.
///
/// P1 ships the reload half. Contract §2.4 also specifies
/// `portal: crossbeam_channel::Sender<PortalRequest>` and
/// `choose_wallpaper(&self, current: Option<PathBuf>)`; those land with the
/// portal worker in P2 (§2.6, and §5's table assigns "§2.6 portal worker | P2"),
/// recorded as deviation P1-D7 and completed per P2-D1.
#[derive(Clone)]
pub struct WorkerHandles {
    reload: crossbeam_channel::Sender<reload::ReloadRequest>,
    portal: crossbeam_channel::Sender<portal::PortalRequest>,
    fs: crossbeam_channel::Sender<fs::FsRequest>,
    /// Whether a worker thread is really behind each sender.
    ///
    /// Both spawns degrade rather than panic under thread exhaustion
    /// (P2-D20), which left Apply and Browse *silently* dead: the channel
    /// accepts the request, nothing ever answers, and the buttons stayed
    /// sensitive with an empty status line. The model reads these to say so
    /// instead.
    reload_alive: bool,
    portal_alive: bool,
    fs_alive: bool,
}

impl WorkerHandles {
    /// Whether an `Apply` will be served by anything.
    #[must_use]
    pub fn reload_available(&self) -> bool {
        self.reload_alive
    }

    /// Whether a Browse click will be served by anything.
    #[must_use]
    pub fn portal_available(&self) -> bool {
        self.portal_alive
    }

    /// Whether wallpaper validation and Revert will be served by anything.
    #[must_use]
    pub fn fs_available(&self) -> bool {
        self.fs_alive
    }

    /// Queue a wallpaper-path check. Never blocks: the answer arrives as
    /// `Msg::WallpaperValidated` on the inbox.
    pub fn validate_wallpaper(&self, text: String) {
        if self
            .fs
            .send(fs::FsRequest::ValidateWallpaper { text })
            .is_err()
        {
            tracing::warn!("the filesystem worker is gone; a wallpaper check was dropped");
        }
    }

    /// Queue a re-read of the config store. Never blocks: the answer arrives
    /// as `Msg::ConfigLoaded` on the inbox.
    pub fn load_config(&self, db_path: std::path::PathBuf) {
        if self.fs.send(fs::FsRequest::LoadConfig { db_path }).is_err() {
            tracing::warn!("the filesystem worker is gone; Revert was dropped");
        }
    }

    /// Queue a save+reload. Never blocks: the answer arrives as
    /// `Msg::Applied` on the inbox. A closed channel (the worker died) is
    /// logged once and dropped — the window stays usable.
    pub fn apply(&self, cfg: icedtea_config::Config, db_path: std::path::PathBuf) {
        if self
            .reload
            .send(reload::ReloadRequest::Apply {
                cfg: Box::new(cfg),
                db_path,
            })
            .is_err()
        {
            tracing::warn!("the reload worker is gone; Apply was dropped");
        }
    }

    /// Queue a file-portal wallpaper pick. Never blocks: the answer arrives
    /// as `Msg::WallpaperChosen` or `Msg::WallpaperPickerFailed` on the
    /// inbox. A closed channel (the worker died) is logged once and dropped.
    pub fn choose_wallpaper(&self, current: Option<std::path::PathBuf>) {
        if self
            .portal
            .send(portal::PortalRequest::OpenFile { current })
            .is_err()
        {
            tracing::warn!("the portal worker is gone; Browse was dropped");
        }
    }
}

/// Handles wired to a receiver the caller keeps, with no worker thread behind
/// them — what a `SettingsModel` unit test constructs (deviation P1-D8).
///
/// P2 extends the returned tuple with the portal receiver, in this same shape
/// (P2-D1).
#[must_use]
pub fn handles_for_test() -> (
    WorkerHandles,
    crossbeam_channel::Receiver<reload::ReloadRequest>,
    crossbeam_channel::Receiver<portal::PortalRequest>,
    crossbeam_channel::Receiver<fs::FsRequest>,
) {
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    let (portal_tx, portal_rx) = crossbeam_channel::unbounded();
    let (fs_tx, fs_rx) = crossbeam_channel::unbounded();
    (
        WorkerHandles {
            reload: reload_tx,
            portal: portal_tx,
            fs: fs_tx,
            reload_alive: true,
            portal_alive: true,
            fs_alive: true,
        },
        reload_rx,
        portal_rx,
        fs_rx,
    )
}

/// [`handles_for_test`]'s handles, but reporting that no worker ever
/// started — what `spawn` produces under thread exhaustion.
///
/// The senders are live and their receivers dropped, exactly as they are in
/// that case: a request is accepted and nothing ever answers it.
#[must_use]
pub fn dead_handles_for_test() -> WorkerHandles {
    let (handles, reload_rx, portal_rx, fs_rx) = handles_for_test();
    drop((reload_rx, portal_rx, fs_rx));
    WorkerHandles {
        reload_alive: false,
        portal_alive: false,
        fs_alive: false,
        ..handles
    }
}

/// The worker threads themselves, kept by `main` for the whole run.
///
/// Separate from [`WorkerHandles`] because a `JoinHandle` is not `Clone` and
/// the handles are: `update` clones a `WorkerHandles` onto every `Cmd::Task`,
/// while exactly one `Workers` exists, in `main`.
pub struct Workers {
    handles: WorkerHandles,
    reload: Option<std::thread::JoinHandle<()>>,
    portal: Option<std::thread::JoinHandle<()>>,
    fs: Option<std::thread::JoinHandle<()>>,
}

/// How long [`Workers::shutdown`] waits for a worker to finish what it is
/// doing. A `redb` commit plus a blocking `ReloadConfig` is the long case.
pub const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl Workers {
    /// The handles the model folds `Cmd::Task`s onto.
    #[must_use]
    pub fn handles(&self) -> WorkerHandles {
        self.handles.clone()
    }

    /// Ask both workers to stop and wait, bounded, for them to.
    ///
    /// Called on the way out of `main`. Apply is asynchronous (P1's
    /// `Cmd::Task` shape), so a click immediately followed by closing the
    /// window used to exit while the reload worker was still inside its
    /// `redb` write: the edit vanished with no message and no error. The
    /// `Shutdown` request is queued *behind* whatever is already in the
    /// channel, so waiting for the worker to reach it is waiting for that
    /// write to land.
    ///
    /// The **portal** worker is told to stop but never waited for. Its
    /// blocking unit of work is a file chooser a human is looking at, bounded
    /// only by `PORTAL_TIMEOUT` (five minutes), so joining it would make
    /// quitting with a chooser open stall for the whole `SHUTDOWN_TIMEOUT`
    /// every time. It holds nothing that has to reach disk — the reason this
    /// function exists at all — and the process is exiting.
    pub fn shutdown(self) {
        let _ = self.handles.reload.send(reload::ReloadRequest::Shutdown);
        let _ = self.handles.portal.send(portal::PortalRequest::Shutdown);
        let _ = self.handles.fs.send(fs::FsRequest::Shutdown);
        drop(self.handles);
        drop(self.portal);
        let deadline = std::time::Instant::now() + SHUTDOWN_TIMEOUT;
        for worker in [self.reload, self.fs] {
            let Some(worker) = worker else { continue };
            while !worker.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                tracing::warn!("a settings worker did not stop in time; exiting without it");
            }
        }
    }
}

use icedtea_ui::view::InboxSender;

/// Start every worker against `tx` and return their handles.
///
/// P1 starts the reload worker; P2 starts the portal worker here too, from
/// the same `tx` (deviation P1-D7, completed per P2-D1); this signature is
/// contract §2.4's final one and does not change when it does.
#[must_use]
pub fn spawn(tx: InboxSender<crate::app::Msg>) -> Workers {
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    let (portal_tx, portal_rx) = crossbeam_channel::unbounded();
    // The join handles are kept, not dropped: `Workers::shutdown` is what
    // makes an Apply survive the window closing under it.
    let (fs_tx, fs_rx) = crossbeam_channel::unbounded();
    let reload = reload::spawn(reload_rx, tx.clone());
    let portal = portal::spawn(portal_rx, tx.clone());
    let fs = fs::spawn(fs_rx, tx);
    Workers {
        handles: WorkerHandles {
            reload: reload_tx,
            portal: portal_tx,
            fs: fs_tx,
            reload_alive: reload.is_some(),
            portal_alive: portal.is_some(),
            fs_alive: fs.is_some(),
        },
        reload,
        portal,
        fs,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// The request round trip, with no D-Bus anywhere near it.
    ///
    /// This used to call the real `spawn`, which opens a session-bus watcher
    /// and blocking-calls `ReloadConfig` on whatever owns
    /// `org.icedtea.Compositor` on the developer's *live* bus — a `cargo test
    /// -p icedtea-settings` forced a real reload of the running compositor and
    /// raced its `ConfigReloaded` against this test's own `Msg::Applied`.
    /// [`super::handles_for_test`] (P1-D8) is what a unit test uses; the
    /// worker's own end-to-end proof is `tests/live_apply.rs`, which runs
    /// against a real compositor deliberately.
    ///
    /// Mutation check: drop the `cfg` from `WorkerHandles::apply`'s request
    /// and this stops compiling; send `default_config()` instead and the
    /// accent assertion fails. Restore.
    #[test]
    fn an_apply_reaches_the_worker_with_the_config_and_the_db_path() {
        let (handles, reload_rx, _portal_rx, _fs_rx) = super::handles_for_test();
        let mut cfg = icedtea_config::default_config();
        cfg.appearance.palette.accent = "#ff00aa".to_string();
        handles.apply(cfg, std::path::PathBuf::from("/tmp/icedtea-test.redb"));

        match reload_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(super::reload::ReloadRequest::Apply { cfg, db_path }) => {
                assert_eq!(cfg.appearance.palette.accent, "#ff00aa");
                assert_eq!(db_path, std::path::PathBuf::from("/tmp/icedtea-test.redb"));
            }
            other => panic!("expected an Apply request, got one: {}", other.is_ok()),
        }
    }

    /// The reload worker really does write the config and answer on the
    /// inbox — over its own channel, with no `ipc::spawn` (so no
    /// `ConfigReloaded` watcher) and against a bus address nothing is
    /// listening on (so no `ReloadConfig` on the developer's live bus, which
    /// would make their running compositor re-read its configuration in the
    /// middle of `cargo test`). The write path is exercised in full; the
    /// D-Bus half fails fast and is asserted as `CompositorAbsent`.
    ///
    /// Mutation check: drop the `tx.send(...)` in
    /// `reload::spawn_worker_on_bus`'s request arm; this test times out and
    /// fails. Restore.
    #[test]
    fn the_reload_worker_writes_the_config_and_answers_on_the_inbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("config.redb");
        let (inbox, tx) = icedtea_ui::view::Inbox::<crate::app::Msg>::new().expect("inbox");
        let (req_tx, req_rx) = crossbeam_channel::unbounded();
        let worker = super::reload::spawn_worker_on_bus(
            req_rx,
            tx,
            Some("unix:path=/nonexistent/icedtea-no-such-bus".to_string()),
        )
        .expect("the reload worker starts");

        let mut cfg = icedtea_config::default_config();
        cfg.appearance.palette.accent = "#ff00aa".to_string();
        req_tx
            .send(super::reload::ReloadRequest::Apply {
                cfg: Box::new(cfg),
                db_path: db.clone(),
            })
            .expect("queue the apply");

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut applied = None;
        while std::time::Instant::now() < deadline && applied.is_none() {
            applied = inbox.try_recv();
            std::thread::sleep(Duration::from_millis(20));
        }
        match applied {
            Some(crate::app::Msg::Applied {
                result: Ok(crate::compositor_reload::ReloadOutcome::CompositorAbsent),
                ..
            }) => {}
            other => panic!("expected Msg::Applied(Ok(CompositorAbsent)), got {other:?}"),
        }
        let on_disk = icedtea_config::load_or_default(&db);
        assert_eq!(on_disk.appearance.palette.accent, "#ff00aa");

        let _ = req_tx.send(super::reload::ReloadRequest::Shutdown);
        worker.join().expect("the worker stops when asked");
    }

    /// A worker that could not be started says so, rather than accepting
    /// requests nothing will ever answer.
    #[test]
    fn handles_report_whether_a_worker_is_behind_them() {
        let (handles, _reload_rx, _portal_rx, _fs_rx) = super::handles_for_test();
        assert!(handles.reload_available());
        assert!(handles.portal_available());
        assert!(handles.fs_available());
    }
}
