//! The client side of the registry.
//!
//! A [`Registry`] is either a **daemon** connection (the production path: the
//! daemon is the sole opener of the file) or a **direct** handle onto an
//! in-process [`Store`] (for tests and for the daemon's own import path, where
//! no bus is involved). Both expose the same method surface, so a consumer is
//! written once and tested without a bus.
//!
//! Values are decoded to [`Value`]s; typed accessors live in the schema crate.
//! [`Registry::watch`] subscribes to the daemon's broadcast `Changed` signal and
//! filters by path prefix client-side; a direct handle cannot observe changes
//! from other processes and so never fires.

use std::sync::{Arc, Mutex};

use zbus::blocking::{Connection, Proxy};

use crate::error::RegistryError;
use crate::store::Store;
use crate::value::{Tag, Value};
use crate::{REGISTRY_BUS_NAME, REGISTRY_IFACE, REGISTRY_PATH};

#[derive(Clone)]
enum Backend {
    Daemon {
        conn: Connection,
        proxy: Proxy<'static>,
    },
    Direct(Arc<Mutex<Store>>),
}

/// A registry handle: daemon-backed in production, direct-backed in tests.
#[derive(Clone)]
pub struct Registry {
    backend: Backend,
}

impl Registry {
    /// Connect to the session bus and bind the registry interface.
    pub fn connect() -> Result<Registry, RegistryError> {
        let conn = Connection::session().map_err(unavailable)?;
        Self::with_connection(conn)
    }

    /// Bind the registry interface over an existing connection.
    pub fn with_connection(conn: Connection) -> Result<Registry, RegistryError> {
        let proxy = Proxy::new(&conn, REGISTRY_BUS_NAME, REGISTRY_PATH, REGISTRY_IFACE)
            .map_err(unavailable)?;
        Ok(Registry {
            backend: Backend::Daemon { conn, proxy },
        })
    }

    /// A handle onto an in-process store. Not for production consumers — the
    /// daemon must be the only opener of the real file — but this is what keeps
    /// consumer unit tests hermetic.
    pub fn direct(store: Arc<Mutex<Store>>) -> Registry {
        Registry {
            backend: Backend::Direct(store),
        }
    }

    fn store(&self) -> Option<std::sync::MutexGuard<'_, Store>> {
        match &self.backend {
            Backend::Direct(store) => Some(store.lock().unwrap_or_else(|e| e.into_inner())),
            Backend::Daemon { .. } => None,
        }
    }

    pub fn get(&self, path: &str) -> Result<(Value, bool), RegistryError> {
        if let Some(store) = self.store() {
            return store.get_effective(path);
        }
        let (tag, bytes, is_default): (u8, Vec<u8>, bool) =
            self.proxy().call("Get", &(path,)).map_err(unavailable)?;
        Ok((decode_wire(tag, &bytes)?, is_default))
    }

    pub fn get_stored(&self, path: &str) -> Result<Option<Value>, RegistryError> {
        if let Some(store) = self.store() {
            return store.get(path);
        }
        let (present, tag, bytes): (bool, u8, Vec<u8>) = self
            .proxy()
            .call("GetStored", &(path,))
            .map_err(unavailable)?;
        if present {
            Ok(Some(decode_wire(tag, &bytes)?))
        } else {
            Ok(None)
        }
    }

    pub fn set(&self, path: &str, value: &Value) -> Result<u64, RegistryError> {
        if let Some(store) = self.store() {
            return store.set(path, value).map(|c| c.seq);
        }
        let (tag, bytes) = encode_wire(value);
        self.proxy()
            .call("Set", &(path, tag, bytes))
            .map_err(unavailable)
    }

    pub fn unset(&self, path: &str) -> Result<u64, RegistryError> {
        if let Some(store) = self.store() {
            return store.unset(path).map(|c| c.seq);
        }
        self.proxy().call("Unset", &(path,)).map_err(unavailable)
    }

    pub fn reset(&self, prefix: &str) -> Result<u64, RegistryError> {
        if let Some(store) = self.store() {
            return store.reset(prefix).map(|(c, _)| c.seq);
        }
        self.proxy().call("Reset", &(prefix,)).map_err(unavailable)
    }

    pub fn set_many(&self, changes: &[(String, Value)]) -> Result<u64, RegistryError> {
        if let Some(store) = self.store() {
            return store.set_many(changes).map(|c| c.seq);
        }
        let wire: Vec<(String, u8, Vec<u8>)> = changes
            .iter()
            .map(|(path, value)| {
                let (tag, bytes) = encode_wire(value);
                (path.clone(), tag, bytes)
            })
            .collect();
        self.proxy().call("SetMany", &(wire,)).map_err(unavailable)
    }

    pub fn unset_many(&self, paths: &[String]) -> Result<u64, RegistryError> {
        if let Some(store) = self.store() {
            return store.unset_many(paths).map(|c| c.seq);
        }
        self.proxy()
            .call("UnsetMany", &(paths,))
            .map_err(unavailable)
    }

    pub fn list(&self, prefix: &str, recurse: bool) -> Result<Vec<(String, Tag)>, RegistryError> {
        if let Some(store) = self.store() {
            return store.list(prefix, recurse);
        }
        let rows: Vec<(String, u8)> = self
            .proxy()
            .call("List", &(prefix, recurse))
            .map_err(unavailable)?;
        rows.into_iter()
            .map(|(path, tag)| {
                Tag::from_u8(tag)
                    .map(|t| (path, t))
                    .ok_or_else(|| RegistryError::Storage("daemon returned an unknown tag".into()))
            })
            .collect()
    }

    pub fn dump(&self, prefix: &str) -> Result<Vec<(String, Value)>, RegistryError> {
        if let Some(store) = self.store() {
            return store.dump(prefix);
        }
        let rows: Vec<(String, u8, Vec<u8>)> =
            self.proxy().call("Dump", &(prefix,)).map_err(unavailable)?;
        rows.into_iter()
            .map(|(path, tag, bytes)| Ok((path, decode_wire(tag, &bytes)?)))
            .collect()
    }

    /// A registered key's spec as JSON, or `None` for an unregistered path.
    pub fn spec_json(&self, path: &str) -> Result<Option<String>, RegistryError> {
        if let Some(store) = self.store() {
            let Some(spec) = store.schema().get(path) else {
                return Ok(None);
            };
            return serde_json::to_string(spec)
                .map(Some)
                .map_err(|e| RegistryError::Storage(format!("spec encode failed: {e}")));
        }
        let result: Result<String, zbus::Error> = self.proxy().call("Spec", &(path,));
        match result {
            Ok(json) => Ok(Some(json)),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.freedesktop.DBus.Error.Failed" =>
            {
                Ok(None)
            }
            Err(err) => Err(unavailable(err)),
        }
    }

    /// The current commit sequence (0 = nothing written yet).
    pub fn seq(&self) -> Result<u64, RegistryError> {
        if let Some(store) = self.store() {
            return store.seq();
        }
        self.proxy().call("Seq", &()).map_err(unavailable)
    }

    /// Subscribe to `Changed` and invoke `on_change` for each broadcast whose
    /// changed-path set intersects `prefix`. Runs on its own thread and returns
    /// immediately. A direct handle cannot see other processes' changes, so it
    /// returns without ever firing.
    pub fn watch<F>(&self, prefix: String, mut on_change: F) -> Result<(), RegistryError>
    where
        F: FnMut(&[(String, bool)], u64) + Send + 'static,
    {
        let Backend::Daemon { conn, .. } = &self.backend else {
            return Ok(());
        };
        let conn = conn.clone();
        std::thread::spawn(move || {
            let Ok(proxy) = Proxy::new(&conn, REGISTRY_BUS_NAME, REGISTRY_PATH, REGISTRY_IFACE)
            else {
                return;
            };
            let Ok(signals) = proxy.receive_signal("Changed") else {
                return;
            };
            for signal in signals {
                let Ok((changes, seq)) = signal.body().deserialize::<(Vec<(String, bool)>, u64)>()
                else {
                    continue;
                };
                let relevant = changes
                    .iter()
                    .any(|(path, _)| crate::path::is_path_prefix(&prefix, path));
                if relevant {
                    on_change(&changes, seq);
                }
            }
        });
        Ok(())
    }

    fn proxy(&self) -> &Proxy<'static> {
        match &self.backend {
            Backend::Daemon { proxy, .. } => proxy,
            Backend::Direct(_) => unreachable!("direct handles never reach the proxy path"),
        }
    }
}

fn encode_wire(value: &Value) -> (u8, Vec<u8>) {
    let mut bytes = Vec::new();
    value.encode(&mut bytes);
    (value.tag().as_u8(), bytes)
}

fn decode_wire(tag: u8, bytes: &[u8]) -> Result<Value, RegistryError> {
    let value = Value::decode(bytes).map_err(|e| RegistryError::Storage(e.to_string()))?;
    if value.tag().as_u8() != tag {
        return Err(RegistryError::Storage(
            "daemon sent a value whose tag disagrees with its payload".into(),
        ));
    }
    Ok(value)
}

fn unavailable(err: zbus::Error) -> RegistryError {
    match err {
        zbus::Error::MethodError(name, detail, _) => {
            let msg = detail
                .map(|d| d.to_string())
                .unwrap_or_else(|| name.to_string());
            if msg.contains("no value at") {
                RegistryError::NotFound(msg)
            } else {
                RegistryError::Unavailable(msg)
            }
        }
        other => RegistryError::Unavailable(other.to_string()),
    }
}
