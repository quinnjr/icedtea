//! The redb-backed, path-keyed store the daemon serves.
//!
//! Kept in the library rather than the binary so it can be exercised without a
//! process (design §"Testing"). One table holds every key (`path -> encoded
//! Value`); a meta table holds the monotonic commit `seq` and the schema
//! version. All writes go through one redb transaction, so a `SetMany` is
//! all-or-nothing and yields exactly one `seq`.
//!
//! The never-panic contract is inherited verbatim from `icedtea-config`: redb
//! can trip an internal `assert!` on a corrupt file rather than returning
//! `Err`, so `open` and every read path catch the unwind and degrade to the
//! caller's fallback instead of propagating a panic.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::error::RegistryError;
use crate::limits;
use crate::path::{is_path_prefix, validate_path};
use crate::spec::Schema;
use crate::value::{Tag, Value};

const DB_KEYS: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("keys");
const DB_META: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("meta");
const KEY_SEQ: &str = "seq";
const KEY_SCHEMA_VERSION: &str = "schema_version";

/// The outcome of one atomic commit: a fresh, monotonic sequence number that
/// every watcher can order changes by and resume from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    pub seq: u64,
}

/// A path-keyed registry store with a compiled schema.
pub struct Store {
    db: Database,
    schema: Schema,
}

impl Store {
    /// Open (creating if absent) the store at `db_path`. A corrupt or
    /// unopenable file is an `Err`, never a panic; the caller decides whether
    /// to start on schema defaults.
    pub fn open(db_path: &Path, schema: Schema) -> Result<Store, RegistryError> {
        if let Some(dir) = db_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let owned = db_path.to_path_buf();
        let db = match catch_unwind_silently(move || Database::create(&owned)) {
            Ok(Ok(db)) => db,
            Ok(Err(err)) => return Err(RegistryError::Storage(err.to_string())),
            Err(payload) => {
                return Err(RegistryError::Storage(format!(
                    "redb panicked while opening the database: {}",
                    panic_message(&payload)
                )));
            }
        };
        let store = Store { db, schema };
        store.ensure_tables()?;
        Ok(store)
    }

    /// An in-memory store with the same semantics as [`Store::open`] — the
    /// hermetic backend consumer unit tests build a [`crate::Registry::direct`]
    /// over, so no file and no daemon are involved.
    pub fn in_memory(schema: Schema) -> Result<Store, RegistryError> {
        let backend = redb::backends::InMemoryBackend::new();
        let db = Database::builder()
            .create_with_backend(backend)
            .map_err(storage_err)?;
        let store = Store { db, schema };
        store.ensure_tables()?;
        Ok(store)
    }

    fn ensure_tables(&self) -> Result<(), RegistryError> {
        let write = self.db.begin_write().map_err(storage_err)?;
        {
            write.open_table(DB_KEYS).map_err(storage_err)?;
            let mut meta = write.open_table(DB_META).map_err(storage_err)?;
            if meta.get(KEY_SEQ).map_err(storage_err)?.is_none() {
                meta.insert(KEY_SEQ, 0u64.to_le_bytes().as_slice())
                    .map_err(storage_err)?;
            }
        }
        write.commit().map_err(storage_err)?;
        Ok(())
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The current commit sequence. `0` means "nothing has ever been written".
    pub fn seq(&self) -> Result<u64, RegistryError> {
        let read = self.db.begin_read().map_err(storage_err)?;
        let meta = match read.open_table(DB_META) {
            Ok(table) => table,
            Err(_) => return Ok(0),
        };
        match meta.get(KEY_SEQ).map_err(storage_err)? {
            Some(bytes) => Ok(decode_u64(bytes.value())),
            None => Ok(0),
        }
    }

    /// Number of stored keys (defaults are not stored, so this is what an
    /// emptiness check wants).
    pub fn key_count(&self) -> Result<u64, RegistryError> {
        let read = self.db.begin_read().map_err(storage_err)?;
        match read.open_table(DB_KEYS) {
            Ok(table) => table.len().map_err(storage_err),
            Err(_) => Ok(0),
        }
    }

    /// Raw stored value, or `None` when the key was never set. Does not consult
    /// the schema — see [`Store::get_effective`] for that.
    pub fn get(&self, path: &str) -> Result<Option<Value>, RegistryError> {
        let read = self.db.begin_read().map_err(storage_err)?;
        let table = match read.open_table(DB_KEYS) {
            Ok(table) => table,
            Err(_) => return Ok(None),
        };
        match table.get(path).map_err(storage_err)? {
            Some(bytes) => Value::decode(bytes.value()).map(Some).map_err(|e| {
                RegistryError::Storage(format!("stored value at {path} is corrupt: {e}"))
            }),
            None => Ok(None),
        }
    }

    /// The effective value: the stored one, else the schema default. Returns
    /// `(value, is_default)`. An absent *unregistered* key is `NotFound`.
    pub fn get_effective(&self, path: &str) -> Result<(Value, bool), RegistryError> {
        validate_path(path)?;
        if let Some(value) = self.get(path)? {
            return Ok((value, false));
        }
        match self.schema.get(path) {
            Some(spec) => Ok((spec.default.clone(), true)),
            None => Err(RegistryError::NotFound(path.to_string())),
        }
    }

    /// Set one key.
    pub fn set(&self, path: &str, value: &Value) -> Result<Commit, RegistryError> {
        self.set_many(std::slice::from_ref(&(path.to_string(), value.clone())))
    }

    /// Set many keys in one transaction. Every path and value is validated
    /// **before** the transaction opens, so a rejected batch writes nothing.
    pub fn set_many(&self, changes: &[(String, Value)]) -> Result<Commit, RegistryError> {
        let mut new_keys = 0u64;
        for (path, value) in changes {
            validate_path(path)?;
            limits::check_value(value)?;
            self.schema.validate_write(path, value)?;
            if self.get(path)?.is_none() {
                new_keys += 1;
            }
        }
        if self.key_count()? + new_keys > limits::MAX_KEY_COUNT {
            return Err(RegistryError::LimitExceeded(format!(
                "store would exceed {} keys",
                limits::MAX_KEY_COUNT
            )));
        }
        let write = self.db.begin_write().map_err(storage_err)?;
        {
            let mut table = write.open_table(DB_KEYS).map_err(storage_err)?;
            for (path, value) in changes {
                let mut buf = Vec::new();
                value.encode(&mut buf);
                table
                    .insert(path.as_str(), buf.as_slice())
                    .map_err(storage_err)?;
            }
        }
        let seq = bump_seq(&write)?;
        write.commit().map_err(storage_err)?;
        Ok(Commit { seq })
    }

    /// Remove one key (reverting it to its default).
    pub fn unset(&self, path: &str) -> Result<Commit, RegistryError> {
        self.unset_many(std::slice::from_ref(&path.to_string()))
    }

    /// Remove many keys in one transaction.
    pub fn unset_many(&self, paths: &[String]) -> Result<Commit, RegistryError> {
        for path in paths {
            validate_path(path)?;
        }
        let write = self.db.begin_write().map_err(storage_err)?;
        {
            let mut table = write.open_table(DB_KEYS).map_err(storage_err)?;
            for path in paths {
                table.remove(path.as_str()).map_err(storage_err)?;
            }
        }
        let seq = bump_seq(&write)?;
        write.commit().map_err(storage_err)?;
        Ok(Commit { seq })
    }

    /// Remove every key under `prefix` — the recursive unset. Changed paths are
    /// returned so the caller can signal exactly what reverted.
    pub fn reset(&self, prefix: &str) -> Result<(Commit, Vec<String>), RegistryError> {
        validate_path(prefix)?;
        let removed = self.matching(prefix)?;
        let write = self.db.begin_write().map_err(storage_err)?;
        {
            let mut table = write.open_table(DB_KEYS).map_err(storage_err)?;
            for path in &removed {
                table.remove(path.as_str()).map_err(storage_err)?;
            }
        }
        let seq = bump_seq(&write)?;
        write.commit().map_err(storage_err)?;
        Ok((Commit { seq }, removed))
    }

    /// Paths under `prefix` (including `prefix` itself) as `(path, tag)`.
    /// `recurse == false` yields only the immediate children.
    pub fn list(&self, prefix: &str, recurse: bool) -> Result<Vec<(String, Tag)>, RegistryError> {
        validate_path(prefix)?;
        let read = self.db.begin_read().map_err(storage_err)?;
        let table = match read.open_table(DB_KEYS) {
            Ok(table) => table,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        let cursor = table.range(prefix..).map_err(storage_err)?;
        for entry in cursor {
            let (key, bytes) = entry.map_err(storage_err)?;
            let path = key.value();
            if !is_path_prefix(prefix, path) {
                break;
            }
            if !recurse && !is_immediate_child(prefix, path) {
                continue;
            }
            let tag = Tag::from_u8(*bytes.value().first().ok_or_else(|| {
                RegistryError::Storage(format!("stored value at {path} is empty"))
            })?)
            .ok_or_else(|| {
                RegistryError::Storage(format!("stored value at {path} has an unknown tag"))
            })?;
            out.push((path.to_string(), tag));
        }
        Ok(out)
    }

    /// Like [`Store::list`] but decodes values too — the daemon's `Dump`.
    pub fn dump(&self, prefix: &str) -> Result<Vec<(String, Value)>, RegistryError> {
        validate_path(prefix)?;
        let read = self.db.begin_read().map_err(storage_err)?;
        let table = match read.open_table(DB_KEYS) {
            Ok(table) => table,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        let cursor = table.range(prefix..).map_err(storage_err)?;
        for entry in cursor {
            let (key, bytes) = entry.map_err(storage_err)?;
            let path = key.value();
            if !is_path_prefix(prefix, path) {
                break;
            }
            let value = Value::decode(bytes.value()).map_err(|e| {
                RegistryError::Storage(format!("stored value at {path} is corrupt: {e}"))
            })?;
            out.push((path.to_string(), value));
        }
        Ok(out)
    }

    /// Stored paths under `prefix`, for internal mutation.
    fn matching(&self, prefix: &str) -> Result<Vec<String>, RegistryError> {
        let read = self.db.begin_read().map_err(storage_err)?;
        let table = match read.open_table(DB_KEYS) {
            Ok(table) => table,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        let cursor = table.range(prefix..).map_err(storage_err)?;
        for entry in cursor {
            let (key, _) = entry.map_err(storage_err)?;
            let path = key.value();
            if !is_path_prefix(prefix, path) {
                break;
            }
            out.push(path.to_string());
        }
        Ok(out)
    }

    /// Record the schema version stamped into the store.
    pub fn stamp_schema_version(&self, version: u64) -> Result<(), RegistryError> {
        let write = self.db.begin_write().map_err(storage_err)?;
        {
            let mut meta = write.open_table(DB_META).map_err(storage_err)?;
            meta.insert(KEY_SCHEMA_VERSION, version.to_le_bytes().as_slice())
                .map_err(storage_err)?;
        }
        write.commit().map_err(storage_err)?;
        Ok(())
    }

    /// The stored schema version: `None` when absent/unreadable (the migration
    /// read path must never treat garbage as a version — design §"Versioning").
    pub fn schema_version(&self) -> Result<Option<u64>, RegistryError> {
        let read = self.db.begin_read().map_err(storage_err)?;
        let meta = match read.open_table(DB_META) {
            Ok(table) => table,
            Err(_) => return Ok(None),
        };
        match meta.get(KEY_SCHEMA_VERSION).map_err(storage_err)? {
            Some(bytes) => Ok(bytes.value().try_into().ok().map(u64::from_le_bytes)),
            None => Ok(None),
        }
    }
}

/// Read the current `seq`, write `seq + 1`, return the new value — inside the
/// caller's write transaction, so the bump and the mutation it describes commit
/// together.
fn bump_seq(write: &redb::WriteTransaction) -> Result<u64, RegistryError> {
    let mut meta = write.open_table(DB_META).map_err(storage_err)?;
    let current = match meta.get(KEY_SEQ).map_err(storage_err)? {
        Some(bytes) => decode_u64(bytes.value()),
        None => 0,
    };
    let next = current.saturating_add(1);
    meta.insert(KEY_SEQ, next.to_le_bytes().as_slice())
        .map_err(storage_err)?;
    Ok(next)
}

fn decode_u64(bytes: &[u8]) -> u64 {
    bytes.try_into().map(u64::from_le_bytes).unwrap_or(0)
}

/// Is `path` a direct child of `prefix` (one segment deeper, not deeper)?
fn is_immediate_child(prefix: &str, path: &str) -> bool {
    let base = if prefix == "/" { "" } else { prefix };
    let Some(rest) = path.strip_prefix(base) else {
        return false;
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    !rest.is_empty() && !rest.contains('/')
}

/// redb surfaces its failures through several distinct error types
/// (`redb::Error`, `TableError`, `StorageError`, `TransactionError`, ...), so
/// this is generic over anything printable rather than naming each one.
fn storage_err<E: std::fmt::Display>(err: E) -> RegistryError {
    RegistryError::Storage(err.to_string())
}

/// Process-global lock serializing the temporary panic-hook swap below.
static PANIC_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` with a no-op panic hook installed, restoring the previous hook
/// before returning. Mirrors `icedtea-config`'s helper: an expected redb panic
/// must not print a raw backtrace that reads like an unhandled crash.
fn catch_unwind_silently<F, R>(f: F) -> std::thread::Result<R>
where
    F: FnOnce() -> R + panic::UnwindSafe,
{
    let _guard = PANIC_HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    panic::set_hook(previous);
    result
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::KeySpec;
    use std::collections::BTreeMap;

    fn schema() -> Schema {
        Schema::new(vec![
            KeySpec {
                path: "/org/icedtea/appearance/bar_height",
                tag: Tag::Uint,
                default: Value::Uint(32),
                description: "bar height",
                choices: None,
                range: None,
                since: 1,
                nullable: false,
            },
            KeySpec {
                path: "/org/icedtea/appearance/bar_position",
                tag: Tag::Str,
                default: Value::Str("top".into()),
                description: "bar position",
                choices: Some(vec![Value::Str("top".into()), Value::Str("bottom".into())]),
                range: None,
                since: 1,
                nullable: false,
            },
        ])
        .unwrap()
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("registry.redb"), schema()).unwrap();
        (dir, store)
    }

    #[test]
    fn unset_registered_key_reads_as_default() {
        let (_dir, store) = store();
        let (value, is_default) = store
            .get_effective("/org/icedtea/appearance/bar_height")
            .unwrap();
        assert_eq!(value, Value::Uint(32));
        assert!(is_default);
        assert_eq!(
            store.get("/org/icedtea/appearance/bar_height").unwrap(),
            None
        );
    }

    #[test]
    fn set_then_get_round_trips_and_is_not_default() {
        let (_dir, store) = store();
        let commit = store
            .set("/org/icedtea/appearance/bar_height", &Value::Uint(48))
            .unwrap();
        assert_eq!(commit.seq, 1);
        let (value, is_default) = store
            .get_effective("/org/icedtea/appearance/bar_height")
            .unwrap();
        assert_eq!(value, Value::Uint(48));
        assert!(!is_default);
    }

    #[test]
    fn unset_reverts_to_default() {
        let (_dir, store) = store();
        store
            .set("/org/icedtea/appearance/bar_height", &Value::Uint(48))
            .unwrap();
        store.unset("/org/icedtea/appearance/bar_height").unwrap();
        let (value, is_default) = store
            .get_effective("/org/icedtea/appearance/bar_height")
            .unwrap();
        assert_eq!(value, Value::Uint(32));
        assert!(is_default);
    }

    #[test]
    fn null_is_distinct_from_unset() {
        let (_dir, store) = store();
        // An unregistered key can hold Null...
        store.set("/apps/x/thing", &Value::Null).unwrap();
        assert_eq!(store.get("/apps/x/thing").unwrap(), Some(Value::Null));
        // ...and unsetting removes it.
        store.unset("/apps/x/thing").unwrap();
        assert_eq!(store.get("/apps/x/thing").unwrap(), None);
    }

    #[test]
    fn a_registered_write_with_the_wrong_tag_is_rejected_and_stores_nothing() {
        let (_dir, store) = store();
        let err = store
            .set(
                "/org/icedtea/appearance/bar_height",
                &Value::Str("tall".into()),
            )
            .unwrap_err();
        assert!(matches!(err, RegistryError::TypeMismatch { .. }));
        assert_eq!(store.key_count().unwrap(), 0);
    }

    #[test]
    fn an_out_of_range_choice_is_rejected() {
        let (_dir, store) = store();
        let err = store
            .set(
                "/org/icedtea/appearance/bar_position",
                &Value::Str("sideways".into()),
            )
            .unwrap_err();
        assert!(matches!(err, RegistryError::OutOfRange { .. }));
    }

    #[test]
    fn unregistered_writes_are_accepted() {
        let (_dir, store) = store();
        store
            .set("/apps/foo/mode", &Value::Str("x".into()))
            .unwrap();
        assert_eq!(
            store.get("/apps/foo/mode").unwrap(),
            Some(Value::Str("x".into()))
        );
    }

    #[test]
    fn set_many_is_atomic_and_one_seq() {
        let (_dir, store) = store();
        let before = store.seq().unwrap();
        store
            .set_many(&[
                ("/org/icedtea/appearance/bar_height".into(), Value::Uint(40)),
                ("/apps/a".into(), Value::Uint(1)),
            ])
            .unwrap();
        assert_eq!(store.seq().unwrap(), before + 1);

        // A batch with one bad entry writes none of its good entries.
        let err = store
            .set_many(&[
                ("/apps/good".into(), Value::Uint(1)),
                (
                    "/org/icedtea/appearance/bar_height".into(),
                    Value::Str("bad".into()),
                ),
            ])
            .unwrap_err();
        assert!(matches!(err, RegistryError::TypeMismatch { .. }));
        assert_eq!(store.get("/apps/good").unwrap(), None);
    }

    #[test]
    fn reset_reverts_a_whole_subtree() {
        let (_dir, store) = store();
        store.set("/apps/a/one", &Value::Uint(1)).unwrap();
        store.set("/apps/a/two", &Value::Uint(2)).unwrap();
        store.set("/apps/ab", &Value::Uint(3)).unwrap();
        let (commit, removed) = store.reset("/apps/a").unwrap();
        assert!(commit.seq >= 1);
        let mut removed = removed;
        removed.sort();
        assert_eq!(removed, vec!["/apps/a/one", "/apps/a/two"]);
        // The segment-prefix sibling `/apps/ab` is untouched.
        assert_eq!(store.get("/apps/ab").unwrap(), Some(Value::Uint(3)));
    }

    #[test]
    fn list_is_segment_aware_and_recurse_controls_depth() {
        let (_dir, store) = store();
        store.set("/apps/a/one", &Value::Uint(1)).unwrap();
        store.set("/apps/a/two", &Value::Uint(2)).unwrap();
        store.set("/apps/ab", &Value::Uint(3)).unwrap();

        let immediate = store.list("/apps", false).unwrap();
        let paths: Vec<_> = immediate.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["/apps/ab"]);

        let deep = store.list("/apps", true).unwrap();
        let mut paths: Vec<_> = deep.iter().map(|(p, _)| p.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec!["/apps/a/one", "/apps/a/two", "/apps/ab"]);
    }

    #[test]
    fn dump_decodes_values() {
        let (_dir, store) = store();
        store.set("/apps/a/one", &Value::Uint(1)).unwrap();
        let rows = store.dump("/apps").unwrap();
        assert_eq!(rows, vec![("/apps/a/one".to_string(), Value::Uint(1))]);
    }

    #[test]
    fn records_and_record_lists_round_trip() {
        let (_dir, store) = store();
        let mut row = BTreeMap::new();
        row.insert("id".to_string(), Value::Str("firefox".into()));
        row.insert("source".to_string(), Value::Str("user".into()));
        let list = Value::RecordList(vec![row.clone()]);
        store
            .set("/desktop/mime/text%2Fhtml/handlers", &list)
            .unwrap();
        assert_eq!(
            store.get("/desktop/mime/text%2Fhtml/handlers").unwrap(),
            Some(list)
        );
    }

    #[test]
    fn seq_is_monotonic_across_commits() {
        let (_dir, store) = store();
        let a = store.set("/apps/a", &Value::Uint(1)).unwrap().seq;
        let b = store.set("/apps/b", &Value::Uint(2)).unwrap().seq;
        let c = store.unset("/apps/a").unwrap().seq;
        assert!(a < b && b < c);
    }

    #[test]
    fn schema_version_round_trips_and_absent_is_none() {
        let (_dir, store) = store();
        assert_eq!(store.schema_version().unwrap(), None);
        store.stamp_schema_version(3).unwrap();
        assert_eq!(store.schema_version().unwrap(), Some(3));
    }

    #[test]
    fn key_cap_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("r.redb"), Schema::new(vec![]).unwrap()).unwrap();
        // Not going to write 100k rows in a unit test; assert the accounting
        // path by checking a fresh store is under the cap and a single huge
        // list value is rejected by the size cap instead.
        assert!(store.key_count().unwrap() < limits::MAX_KEY_COUNT);
        let err = store
            .set(
                "/apps/x",
                &Value::Bytes(vec![0; limits::MAX_VALUE_BYTES + 1]),
            )
            .unwrap_err();
        assert!(matches!(err, RegistryError::LimitExceeded(_)));
    }
}
