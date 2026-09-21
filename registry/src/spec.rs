//! Key declarations ([`KeySpec`]) and the compiled schema ([`Schema`]).
//!
//! A component's schema is a plain `Vec<KeySpec>` built at runtime (defaults
//! are real [`Value`]s, which is why this is a function rather than a `const`).
//! The daemon compiles every first-party domain's specs into one [`Schema`] at
//! boot and validates writes against it; a component compiles the same specs
//! for its typed accessors. Because both build it from the same crate, they can
//! never disagree about a key's type or default.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::RegistryError;
use crate::path::validate_path;
use crate::value::{Tag, Value};

/// One declared key: its type, its default, and enough metadata to render and
/// validate it. `path` is a literal so a schema is zero-allocation at rest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeySpec {
    /// Absolute canonical path (see [`crate::path`]).
    pub path: &'static str,
    /// The one tag this key accepts.
    pub tag: Tag,
    /// Effective value when the key is not stored. Must match `tag`, and must
    /// satisfy `choices`/`range` when those are set.
    pub default: Value,
    /// Human description, surfaced by `Spec(path)` for settings UIs.
    pub description: &'static str,
    /// If set, the only accepted values (checked before `range`).
    pub choices: Option<Vec<Value>>,
    /// If set, an inclusive numeric bound for `Int`/`Uint`/`Double`.
    pub range: Option<(Value, Value)>,
    /// Schema revision that introduced the key, for migration bookkeeping.
    pub since: u64,
    /// When true, a write of [`Value::Null`] is also accepted in addition to
    /// `tag`, so an `Option`-typed setting can express "explicitly empty"
    /// against a non-null tag (design §"Unset vs default"). `default` may then
    /// be `Null` even though `tag` is something else.
    pub nullable: bool,
}

/// The compiled union of every linked domain's [`KeySpec`]s.
#[derive(Debug, Clone)]
pub struct Schema {
    specs: Vec<KeySpec>,
    by_path: BTreeMap<&'static str, usize>,
}

impl Schema {
    /// Compile a spec list. Fails (rather than warns) on a malformed path, a
    /// default that does not match its tag, or — critically — a **duplicate
    /// path**: two crates declaring the same key is a build defect that must
    /// stop the daemon from starting (design §Risks, "schema drift").
    pub fn new(specs: Vec<KeySpec>) -> Result<Schema, RegistryError> {
        let mut by_path = BTreeMap::new();
        for (index, spec) in specs.iter().enumerate() {
            validate_path(spec.path).map_err(|e| {
                RegistryError::Schema(format!("spec {:?} has an invalid path: {e}", spec.path))
            })?;
            let default_ok = spec.default.tag() == spec.tag
                || (spec.nullable && spec.default.tag() == Tag::Null);
            if !default_ok {
                return Err(RegistryError::Schema(format!(
                    "spec {:?} declares tag {} but its default is {}",
                    spec.path,
                    spec.tag.name(),
                    spec.default.tag().name()
                )));
            }
            if let Some((lo, hi)) = &spec.range
                && compare(lo, hi)? == std::cmp::Ordering::Greater
            {
                return Err(RegistryError::Schema(format!(
                    "spec {:?} has a range whose low bound exceeds its high bound",
                    spec.path
                )));
            }
            if let Some(prev) = by_path.insert(spec.path, index) {
                return Err(RegistryError::Schema(format!(
                    "path {:?} is declared twice (specs {prev} and {index})",
                    spec.path
                )));
            }
        }
        Ok(Schema { specs, by_path })
    }

    /// The spec for a registered path, or `None` for an open (unregistered)
    /// path.
    pub fn get(&self, path: &str) -> Option<&KeySpec> {
        self.by_path.get(path).map(|i| &self.specs[*i])
    }

    pub fn specs(&self) -> &[KeySpec] {
        &self.specs
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Validate a candidate write. For a registered key: the tag must match and
    /// `choices`/`range` must hold. For an unregistered key: any well-formed
    /// value is accepted (design Q8 — the open fallback).
    pub fn validate_write(&self, path: &str, value: &Value) -> Result<(), RegistryError> {
        let Some(spec) = self.get(path) else {
            return Ok(());
        };
        let tag_ok = value.tag() == spec.tag || (spec.nullable && value.tag() == Tag::Null);
        if !tag_ok {
            return Err(RegistryError::TypeMismatch {
                path: path.to_string(),
                expected: spec.tag,
                got: value.tag(),
            });
        }
        // `Null` is a legitimate nullable value but has no choices/range.
        if value.tag() == Tag::Null {
            return Ok(());
        }
        if let Some(choices) = &spec.choices
            && !choices.contains(value)
        {
            return Err(RegistryError::OutOfRange {
                path: path.to_string(),
                reason: "value is not one of the allowed choices".to_string(),
            });
        }
        if let Some((lo, hi)) = &spec.range
            && (compare(value, lo)? == std::cmp::Ordering::Less
                || compare(value, hi)? == std::cmp::Ordering::Greater)
        {
            return Err(RegistryError::OutOfRange {
                path: path.to_string(),
                reason: format!(
                    "value is outside the allowed range [{}..{}]",
                    lo.tag().name(),
                    hi.tag().name()
                ),
            });
        }
        Ok(())
    }
}

/// Numerically compare two values, used by both the range check and the
/// range-bound sanity check at compile time. Refuses non-numeric operands
/// rather than guessing.
fn compare(a: &Value, b: &Value) -> Result<std::cmp::Ordering, RegistryError> {
    use std::cmp::Ordering;
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Uint(x), Value::Uint(y)) => x.cmp(y),
        (Value::Double(x), Value::Double(y)) => x.partial_cmp(y).unwrap_or(Ordering::Less),
        _ => {
            return Err(RegistryError::Schema(
                "range bounds must share one numeric type".into(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(path: &'static str, tag: Tag, default: Value) -> KeySpec {
        KeySpec {
            path,
            tag,
            default,
            description: "",
            choices: None,
            range: None,
            since: 1,
            nullable: false,
        }
    }

    #[test]
    fn a_nullable_key_accepts_null_and_may_default_to_null() {
        let mut s = spec("/org/icedtea/appearance/wallpaper", Tag::Str, Value::Null);
        s.nullable = true;
        let schema = Schema::new(vec![s]).unwrap();
        assert!(
            schema
                .validate_write("/org/icedtea/appearance/wallpaper", &Value::Null)
                .is_ok()
        );
        assert!(
            schema
                .validate_write(
                    "/org/icedtea/appearance/wallpaper",
                    &Value::Str("/w.png".into())
                )
                .is_ok()
        );
        // A non-nullable key still refuses Null.
        let schema = Schema::new(vec![spec("/a/b", Tag::Str, Value::Str(String::new()))]).unwrap();
        assert!(matches!(
            schema.validate_write("/a/b", &Value::Null),
            Err(RegistryError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn duplicate_paths_are_a_hard_error() {
        let err = Schema::new(vec![
            spec("/a/b", Tag::Uint, Value::Uint(1)),
            spec("/a/b", Tag::Uint, Value::Uint(2)),
        ])
        .unwrap_err();
        assert!(matches!(err, RegistryError::Schema(_)));
        assert!(err.to_string().contains("declared twice"));
    }

    #[test]
    fn a_default_that_mismatches_its_tag_is_a_hard_error() {
        let err = Schema::new(vec![spec("/a/b", Tag::Uint, Value::Str("x".into()))]).unwrap_err();
        assert!(err.to_string().contains("declares tag uint"));
    }

    #[test]
    fn an_invalid_path_is_a_hard_error() {
        let err = Schema::new(vec![spec("a/b", Tag::Uint, Value::Uint(1))]).unwrap_err();
        assert!(err.to_string().contains("invalid path"));
    }

    #[test]
    fn registered_writes_are_type_checked() {
        let schema = Schema::new(vec![spec("/a/b", Tag::Uint, Value::Uint(1))]).unwrap();
        assert!(schema.validate_write("/a/b", &Value::Uint(9)).is_ok());
        let err = schema
            .validate_write("/a/b", &Value::Str("x".into()))
            .unwrap_err();
        assert!(matches!(
            err,
            RegistryError::TypeMismatch {
                expected: Tag::Uint,
                got: Tag::Str,
                ..
            }
        ));
    }

    #[test]
    fn unregistered_writes_are_accepted() {
        let schema = Schema::new(vec![spec("/a/b", Tag::Uint, Value::Uint(1))]).unwrap();
        assert!(
            schema
                .validate_write("/any/thing", &Value::Str("x".into()))
                .is_ok()
        );
        assert!(
            schema
                .validate_write("/any/thing", &Value::Record(BTreeMap::new()))
                .is_ok()
        );
    }

    #[test]
    fn choices_are_enforced() {
        let mut s = spec("/a/mode", Tag::Str, Value::Str("on".into()));
        s.choices = Some(vec![Value::Str("on".into()), Value::Str("off".into())]);
        let schema = Schema::new(vec![s]).unwrap();
        assert!(
            schema
                .validate_write("/a/mode", &Value::Str("off".into()))
                .is_ok()
        );
        assert!(matches!(
            schema.validate_write("/a/mode", &Value::Str("maybe".into())),
            Err(RegistryError::OutOfRange { .. })
        ));
    }

    #[test]
    fn ranges_are_enforced_inclusively() {
        let mut s = spec("/a/n", Tag::Uint, Value::Uint(4));
        s.range = Some((Value::Uint(1), Value::Uint(8)));
        let schema = Schema::new(vec![s]).unwrap();
        assert!(schema.validate_write("/a/n", &Value::Uint(1)).is_ok());
        assert!(schema.validate_write("/a/n", &Value::Uint(8)).is_ok());
        assert!(matches!(
            schema.validate_write("/a/n", &Value::Uint(9)),
            Err(RegistryError::OutOfRange { .. })
        ));
    }

    #[test]
    fn a_reversed_range_is_a_hard_error() {
        let mut s = spec("/a/n", Tag::Uint, Value::Uint(4));
        s.range = Some((Value::Uint(8), Value::Uint(1)));
        assert!(Schema::new(vec![s]).is_err());
    }

    #[test]
    fn lookup_and_listing() {
        let schema = Schema::new(vec![
            spec("/a/b", Tag::Uint, Value::Uint(1)),
            spec("/a/c", Tag::Str, Value::Str(String::new())),
        ])
        .unwrap();
        assert_eq!(schema.get("/a/b").map(|s| s.tag), Some(Tag::Uint));
        assert_eq!(schema.get("/nope"), None);
        assert_eq!(schema.specs().len(), 2);
    }
}
