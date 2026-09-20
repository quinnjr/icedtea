//! `icedtea-registry` — the icedtea configuration *and* registration registry.
//!
//! See `docs/superpowers/specs/2026-09-20-icedtea-registry-design.md`. This
//! crate is three things:
//!
//! * the vocabulary — [`Value`], [`Tag`], [`KeySpec`], [`Schema`], the path
//!   grammar and the error set, used by the daemon, every consumer, and the
//!   CLI alike;
//! * the client — [`client::Registry`] over `org.icedtea.Registry`, with
//!   typed accessors and watch handles;
//! * the store — the redb-backed, path-keyed implementation the daemon serves,
//!   kept here (not in the binary) so it is testable without a process.
//!
//! The schema crate (`icedtea-registry-schema`) depends on this one; this one
//! never depends on it, so the daemon can link both without a cycle.

pub mod client;
pub mod error;
pub mod limits;
pub mod path;
pub mod service;
pub mod spec;
pub mod store;
pub mod value;

pub use client::Registry;
pub use error::RegistryError;
pub use path::{
    app_path, is_path_prefix, mime_handlers_path, mime_path, mime_removed_path,
    scheme_handlers_path, scheme_path, validate_path,
};
pub use service::{Change, SpawnError};
pub use spec::{KeySpec, Schema};
pub use store::{Commit, Store};
pub use value::{DecodeError, Tag, Value};

/// Well-known bus name and object path for the registry daemon. These are the
/// one place the literals live (mirroring `icedtea-contract`'s convention).
pub const REGISTRY_BUS_NAME: &str = "org.icedtea.Registry";
pub const REGISTRY_PATH: &str = "/org/icedtea/Registry";
pub const REGISTRY_IFACE: &str = "org.icedtea.Registry";

/// Current schema revision. Bumped when a later migration needs a new fixed
/// boundary (design §"Versioning"); the importer's A3 gate is a separate,
/// frozen constant, so bumping this never re-arms an old migration.
pub const SCHEMA_VERSION: u64 = 1;

/// `$XDG_CONFIG_HOME/icedtea/registry.redb`, falling back to
/// `~/.config/icedtea/registry.redb` — the same base the old config used, with
/// a new filename so both can coexist during the import.
pub fn default_db_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().expect("HOME set").join(".config"));
    base.join("icedtea").join("registry.redb")
}
