//! The reload worker: one OS thread owning the blocking `ReloadClient` for
//! outbound `ReloadConfig` calls and a second, async connection subscribed to
//! the compositor's `ConfigReloaded` signal.
//!
//! `update` never blocks and never calls D-Bus (spec D8), so every save+reload
//! arrives here as a [`ReloadRequest`] and every answer leaves as a
//! `Msg::Applied` on the inbox.

/// What the loop thread asks the worker for.
pub enum ReloadRequest {
    /// Write `cfg` to `db_path`, then best-effort `ReloadConfig`.
    Apply {
        cfg: icedtea_config::Config,
        db_path: std::path::PathBuf,
    },
}
