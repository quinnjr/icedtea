//! `icedtea-registryd` — the registry broker.
//!
//! Sole opener of `registry.redb`; serves `org.icedtea.Registry`. On first boot
//! with a `--import <config.redb>` argument (or when the old file is found and
//! the registry is empty), it migrates the pre-registry config once, renames it
//! aside, and then serves.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use icedtea_registry::service::{self, SpawnError};
use icedtea_registry::store::Store;
use icedtea_registry::{SCHEMA_VERSION, default_db_path};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let import_from = match parse_args(&args) {
        Ok(path) => path,
        Err(msg) => {
            eprintln!("icedtea-registryd: {msg}");
            eprintln!("usage: icedtea-registryd [--import <config.redb>] [--db <registry.redb>]");
            std::process::exit(2);
        }
    };

    let db_path = args
        .windows(2)
        .find(|w| w[0] == "--db")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(default_db_path);

    let store = match Store::open(&db_path, icedtea_registry_schema::schema()) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "icedtea-registryd: could not open {}: {err}",
                db_path.display()
            );
            std::process::exit(1);
        }
    };

    if let Some(config_db) = import_from.as_deref()
        && let Err(err) = import_once(&store, config_db)
    {
        eprintln!(
            "icedtea-registryd: import from {} failed: {err}",
            config_db.display()
        );
        std::process::exit(1);
    }

    let store = Arc::new(Mutex::new(store));
    let (changes_tx, changes_rx) = crossbeam_channel::unbounded();

    let _dbus = match service::spawn(store, changes_rx, changes_tx) {
        Ok(conn) => conn,
        Err(SpawnError::NameTaken(msg)) => {
            // A second registry daemon is already serving. Fatal but clean:
            // exit 0 so systemd does not respawn us into a crash loop.
            eprintln!("icedtea-registryd: {msg}");
            std::process::exit(0);
        }
        Err(SpawnError::Bus(err)) => {
            eprintln!("icedtea-registryd: fatal D-Bus error: {err}");
            std::process::exit(1);
        }
    };

    loop {
        std::thread::park();
    }
}

fn parse_args(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut import = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--import" => {
                let path = args.get(i + 1).ok_or("--import needs a path")?;
                import = Some(PathBuf::from(path));
                i += 2;
            }
            "--db" => {
                args.get(i + 1).ok_or("--db needs a path")?;
                i += 2;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(import)
}

/// Migrate the old config once, only into an empty registry, and rename the old
/// file aside rather than deleting it. Re-running is a no-op.
fn import_once(store: &Store, config_db: &Path) -> Result<(), icedtea_registry::RegistryError> {
    if store.key_count()? > 0 {
        return Ok(());
    }
    let entries = icedtea_registry_schema::import::import_config_db(config_db)?;
    if !entries.is_empty() {
        store.set_many(&entries)?;
    }
    store.stamp_schema_version(SCHEMA_VERSION)?;
    let migrated = config_db.with_extension("redb.migrated");
    if let Err(err) = std::fs::rename(config_db, &migrated) {
        tracing::warn!(
            error = %err,
            "imported the old config but could not rename it aside"
        );
    }
    Ok(())
}
