//! The registry's error vocabulary, shared by the daemon and the client so a
//! consumer can match on a failure instead of parsing a string.

use crate::value::Tag;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The path did not match the grammar ([`crate::path::validate_path`]).
    InvalidPath(String),
    /// A write to a registered key carried the wrong tag.
    TypeMismatch {
        path: String,
        expected: Tag,
        got: Tag,
    },
    /// `Get` on an absent, unregistered key.
    NotFound(String),
    /// A value outside its spec's `choices`/`range`.
    OutOfRange { path: String, reason: String },
    /// A resource cap was exceeded (depth, size, count, list length).
    LimitExceeded(String),
    /// Two specs declared one path, or a spec was internally inconsistent —
    /// always a build defect, never a runtime input error.
    Schema(String),
    /// The backing store failed (open, read, write).
    Storage(String),
    /// The daemon is not reachable.
    Unavailable(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::InvalidPath(msg) => write!(f, "invalid path: {msg}"),
            RegistryError::TypeMismatch {
                path,
                expected,
                got,
            } => write!(
                f,
                "type mismatch at {path}: expected {}, got {}",
                expected.name(),
                got.name()
            ),
            RegistryError::NotFound(path) => write!(f, "no value at {path}"),
            RegistryError::OutOfRange { path, reason } => {
                write!(f, "value rejected at {path}: {reason}")
            }
            RegistryError::LimitExceeded(msg) => write!(f, "limit exceeded: {msg}"),
            RegistryError::Schema(msg) => write!(f, "schema error: {msg}"),
            RegistryError::Storage(msg) => write!(f, "storage error: {msg}"),
            RegistryError::Unavailable(msg) => write!(f, "registry unavailable: {msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_failure() {
        assert_eq!(
            RegistryError::NotFound("/a".into()).to_string(),
            "no value at /a"
        );
        assert_eq!(
            RegistryError::TypeMismatch {
                path: "/a".into(),
                expected: Tag::Uint,
                got: Tag::Str,
            }
            .to_string(),
            "type mismatch at /a: expected uint, got str"
        );
    }
}
