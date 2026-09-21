//! Resource caps. These exist so a runaway or malicious writer gets an error
//! instead of a full disk or an OOM'd daemon (design §"Security and trust
//! posture"). They are enforced by the daemon on every commit and are the only
//! thing standing between an open write model and a trivially-DoS-able one.

use crate::error::RegistryError;
use crate::value::Value;

/// Largest accepted encoded value (the design's ~1 MiB cap).
pub const MAX_VALUE_BYTES: usize = 1 << 20;
/// Largest accepted number of keys in the whole store.
pub const MAX_KEY_COUNT: u64 = 100_000;
/// Largest accepted `StrList`/`RecordList` length.
pub const MAX_LIST_LEN: usize = 4096;
/// Largest accepted number of fields in one `Record`.
pub const MAX_RECORD_FIELDS: usize = 256;

/// Reject a value that breaks a structural cap. Size is measured against the
/// canonical encoding, never a self-reported length.
pub fn check_value(value: &Value) -> Result<(), RegistryError> {
    if value.encoded_len() > MAX_VALUE_BYTES {
        return Err(RegistryError::LimitExceeded(format!(
            "value encodes to more than {MAX_VALUE_BYTES} bytes"
        )));
    }
    match value {
        Value::StrList(list) if list.len() > MAX_LIST_LEN => Err(RegistryError::LimitExceeded(
            format!("list longer than {MAX_LIST_LEN}"),
        )),
        Value::Record(map) if map.len() > MAX_RECORD_FIELDS => Err(RegistryError::LimitExceeded(
            format!("record with more than {MAX_RECORD_FIELDS} fields"),
        )),
        Value::RecordList(rows) if rows.len() > MAX_LIST_LEN => Err(RegistryError::LimitExceeded(
            format!("record list longer than {MAX_LIST_LEN}"),
        )),
        Value::RecordList(rows) => {
            for row in rows {
                if row.len() > MAX_RECORD_FIELDS {
                    return Err(RegistryError::LimitExceeded(format!(
                        "record with more than {MAX_RECORD_FIELDS} fields"
                    )));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn small_values_pass() {
        assert!(check_value(&Value::Uint(1)).is_ok());
        assert!(check_value(&Value::Str("hello".into())).is_ok());
    }

    #[test]
    fn oversize_bytes_are_rejected() {
        let big = Value::Bytes(vec![0u8; MAX_VALUE_BYTES + 1]);
        assert!(matches!(
            check_value(&big),
            Err(RegistryError::LimitExceeded(_))
        ));
    }

    #[test]
    fn oversize_lists_are_rejected() {
        let list = Value::StrList(vec!["x".to_string(); MAX_LIST_LEN + 1]);
        assert!(matches!(
            check_value(&list),
            Err(RegistryError::LimitExceeded(_))
        ));
    }

    #[test]
    fn oversize_records_are_rejected() {
        let mut map = BTreeMap::new();
        for i in 0..=MAX_RECORD_FIELDS {
            map.insert(format!("k{i}"), Value::Uint(0));
        }
        assert!(matches!(
            check_value(&Value::Record(map)),
            Err(RegistryError::LimitExceeded(_))
        ));
    }
}
