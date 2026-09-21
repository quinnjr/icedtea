//! The `org.icedtea.Registry` D-Bus service the daemon serves.
//!
//! Values cross the bus as `(y tag, ay payload)` using the canonical codec in
//! [`crate::value`] — one codec for disk and wire. (The design spec sketched a
//! `zvariant` variant payload; the tag stays on the wire so a generic client
//! can still tell a key's type, and the payload is the same bytes the store
//! holds, which removes a whole second encoding to keep honest.)
//!
//! Every mutation commits through [`Store`] and then pushes exactly one
//! [`Change`] onto a channel; the emitter thread turns that into a single
//! broadcast `Changed(changes, seq)` signal. Watchers filter by prefix
//! client-side (see [`crate::client`]) — the daemon tracks no per-connection
//! state, so a watcher that dies leaks nothing.

use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use zbus::blocking::Connection;
use zbus::interface;

use crate::error::RegistryError;
use crate::store::{Commit, Store};
use crate::value::{Tag, Value};
use crate::{REGISTRY_BUS_NAME, REGISTRY_IFACE, REGISTRY_PATH};

/// One committed mutation, awaiting emission.
#[derive(Debug, Clone)]
pub struct Change {
    pub seq: u64,
    /// `(path, present)` — `present == false` means the key was unset/reverted.
    pub changes: Vec<(String, bool)>,
}

/// Why the daemon could not start.
#[derive(Debug)]
pub enum SpawnError {
    /// Another process owns `org.icedtea.Registry`. Fatal but expected: exit
    /// cleanly, never steal the name.
    NameTaken(String),
    /// A genuine D-Bus fault.
    Bus(zbus::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::NameTaken(msg) => write!(f, "{msg}"),
            SpawnError::Bus(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SpawnError {}

impl From<zbus::Error> for SpawnError {
    fn from(err: zbus::Error) -> Self {
        SpawnError::Bus(err)
    }
}

/// The interface object. Methods are synchronous (matching the notifications
/// daemon): redb operations are short, so blocking the executor briefly is
/// preferable to a second async code path.
pub struct RegistryIface {
    store: Arc<Mutex<Store>>,
    changes: Sender<Change>,
}

impl RegistryIface {
    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn commit(
        &self,
        result: Result<(Commit, Vec<(String, bool)>), RegistryError>,
    ) -> Result<u64, zbus::fdo::Error> {
        let (commit, changes) = result.map_err(to_fdo)?;
        let _ = self.changes.send(Change {
            seq: commit.seq,
            changes,
        });
        Ok(commit.seq)
    }
}

fn to_fdo(err: RegistryError) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(err.to_string())
}

fn decode_checked(value: &[u8], tag: u8) -> Result<Value, RegistryError> {
    let decoded = Value::decode(value).map_err(|e| RegistryError::Storage(e.to_string()))?;
    if decoded.tag().as_u8() != tag {
        return Err(RegistryError::TypeMismatch {
            path: "<wire>".into(),
            expected: Tag::from_u8(tag).unwrap_or(Tag::Null),
            got: decoded.tag(),
        });
    }
    Ok(decoded)
}

#[interface(name = "org.icedtea.Registry")]
impl RegistryIface {
    /// Effective value at `path` (stored, else schema default), with a flag
    /// saying which it was.
    fn get(&self, path: String) -> Result<(u8, Vec<u8>, bool), zbus::fdo::Error> {
        let (value, is_default) = self.store().get_effective(&path).map_err(to_fdo)?;
        let mut bytes = Vec::new();
        value.encode(&mut bytes);
        Ok((value.tag().as_u8(), bytes, is_default))
    }

    /// The stored value at `path`, if any — "did the user set this?".
    fn get_stored(&self, path: String) -> Result<(bool, u8, Vec<u8>), zbus::fdo::Error> {
        match self.store().get(&path).map_err(to_fdo)? {
            Some(value) => {
                let mut bytes = Vec::new();
                value.encode(&mut bytes);
                Ok((true, value.tag().as_u8(), bytes))
            }
            None => Ok((false, Tag::Null.as_u8(), Vec::new())),
        }
    }

    /// Set one key. `tag` must agree with the encoding of `value`.
    fn set(&self, path: String, tag: u8, value: Vec<u8>) -> Result<u64, zbus::fdo::Error> {
        let decoded = decode_checked(&value, tag).map_err(to_fdo)?;
        let commit = self.store().set(&path, &decoded).map_err(to_fdo)?;
        let _ = self.changes.send(Change {
            seq: commit.seq,
            changes: vec![(path, true)],
        });
        Ok(commit.seq)
    }

    /// Remove one key.
    fn unset(&self, path: String) -> Result<u64, zbus::fdo::Error> {
        let commit = self.store().unset(&path).map_err(to_fdo)?;
        let _ = self.changes.send(Change {
            seq: commit.seq,
            changes: vec![(path, false)],
        });
        Ok(commit.seq)
    }

    /// Recursively revert a subtree.
    fn reset(&self, prefix: String) -> Result<u64, zbus::fdo::Error> {
        self.commit((|| {
            let (commit, removed) = self.store().reset(&prefix)?;
            let changes = removed.into_iter().map(|p| (p, false)).collect();
            Ok((commit, changes))
        })())
    }

    /// Atomic batch set — one transaction, one `seq`, one `Changed`.
    fn set_many(&self, changes: Vec<(String, u8, Vec<u8>)>) -> Result<u64, zbus::fdo::Error> {
        let store = self.store();
        let mut decoded = Vec::with_capacity(changes.len());
        for (path, tag, value) in &changes {
            decoded.push((path.clone(), decode_checked(value, *tag).map_err(to_fdo)?));
        }
        let commit = store.set_many(&decoded).map_err(to_fdo)?;
        let paths = changes.into_iter().map(|(p, _, _)| (p, true)).collect();
        drop(store);
        let _ = self.changes.send(Change {
            seq: commit.seq,
            changes: paths,
        });
        Ok(commit.seq)
    }

    /// Atomic batch unset.
    fn unset_many(&self, paths: Vec<String>) -> Result<u64, zbus::fdo::Error> {
        let commit = self.store().unset_many(&paths).map_err(to_fdo)?;
        let changes = paths.into_iter().map(|p| (p, false)).collect();
        let _ = self.changes.send(Change {
            seq: commit.seq,
            changes,
        });
        Ok(commit.seq)
    }

    /// `(path, tag)` for keys under `prefix`; `recurse == false` for immediate
    /// children only.
    fn list(&self, prefix: String, recurse: bool) -> Result<Vec<(String, u8)>, zbus::fdo::Error> {
        let rows = self.store().list(&prefix, recurse).map_err(to_fdo)?;
        Ok(rows.into_iter().map(|(p, t)| (p, t.as_u8())).collect())
    }

    /// `(path, tag, value)` for every key under `prefix`.
    fn dump(&self, prefix: String) -> Result<Vec<(String, u8, Vec<u8>)>, zbus::fdo::Error> {
        let rows = self.store().dump(&prefix).map_err(to_fdo)?;
        Ok(rows
            .into_iter()
            .map(|(p, v)| {
                let mut bytes = Vec::new();
                v.encode(&mut bytes);
                (p, v.tag().as_u8(), bytes)
            })
            .collect())
    }

    /// The current commit sequence (0 = nothing written yet).
    fn seq(&self) -> Result<u64, zbus::fdo::Error> {
        self.store().seq().map_err(to_fdo)
    }

    /// A registered key's spec, JSON-encoded (description, tag, default,
    /// choices, range, since). `NotFound` for an unregistered path.
    fn spec(&self, path: String) -> Result<String, zbus::fdo::Error> {
        let store = self.store();
        let Some(spec) = store.schema().get(&path) else {
            return Err(zbus::fdo::Error::Failed(
                RegistryError::NotFound(path).to_string(),
            ));
        };
        serde_json::to_string(spec)
            .map_err(|e| zbus::fdo::Error::Failed(format!("spec encode failed: {e}")))
    }
}

/// Register the interface, claim [`REGISTRY_BUS_NAME`], and start the emitter
/// thread. Name conflict is fatal-but-clean (mirrors the notifications daemon).
pub fn spawn(
    store: Arc<Mutex<Store>>,
    changes_rx: crossbeam_channel::Receiver<Change>,
    changes_tx: Sender<Change>,
) -> Result<Connection, SpawnError> {
    let conn = Connection::session()?;
    let iface = RegistryIface {
        store,
        changes: changes_tx,
    };
    conn.object_server()
        .at(REGISTRY_PATH, iface)
        .map_err(SpawnError::Bus)?;

    let name_taken = || {
        SpawnError::NameTaken(format!(
            "another registry daemon already owns {REGISTRY_BUS_NAME}"
        ))
    };
    match conn.request_name_with_flags(
        REGISTRY_BUS_NAME,
        zbus::fdo::RequestNameFlags::DoNotQueue.into(),
    ) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {}
        Ok(_) => return Err(name_taken()),
        Err(zbus::Error::NameTaken) => return Err(name_taken()),
        Err(err) => return Err(SpawnError::Bus(err)),
    }

    let emitter = conn.clone();
    std::thread::spawn(move || {
        while let Ok(change) = changes_rx.recv() {
            if let Err(err) = emitter.emit_signal(
                None::<&str>,
                REGISTRY_PATH,
                REGISTRY_IFACE,
                "Changed",
                &(change.changes, change.seq),
            ) {
                tracing::warn!(error = %err, "failed to emit registry Changed");
            }
        }
    });

    Ok(conn)
}
