//! The registry's value model and its on-the-wire/on-disk codec.
//!
//! Values are a small, closed, tagged set (design §"Values"). Every value is
//! encoded as a one-byte [`Tag`] followed by a payload; the same bytes are
//! stored in redb and carried as the `ay` payload of every D-Bus message, so
//! there is exactly one codec to keep honest.
//!
//! `Record`/`RecordList` are one level deep by construction: a record's values
//! must all be *scalars* ([`Value::is_scalar`]), which the daemon enforces on
//! every write. That is what lets a generic tool render any record without
//! recursing without bound.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The closed value type set. The numeric discriminants are the on-disk/wire
/// tag bytes and must never change for an existing variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Tag {
    Str = 0,
    Int = 1,
    Uint = 2,
    Bool = 3,
    Double = 4,
    Bytes = 5,
    StrList = 6,
    Null = 7,
    Record = 8,
    RecordList = 9,
}

impl Tag {
    pub fn from_u8(byte: u8) -> Option<Tag> {
        Some(match byte {
            0 => Tag::Str,
            1 => Tag::Int,
            2 => Tag::Uint,
            3 => Tag::Bool,
            4 => Tag::Double,
            5 => Tag::Bytes,
            6 => Tag::StrList,
            7 => Tag::Null,
            8 => Tag::Record,
            9 => Tag::RecordList,
            _ => return None,
        })
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// The design's human name for the tag, used in errors and `Spec` replies.
    pub fn name(self) -> &'static str {
        match self {
            Tag::Str => "str",
            Tag::Int => "int",
            Tag::Uint => "uint",
            Tag::Bool => "bool",
            Tag::Double => "double",
            Tag::Bytes => "bytes",
            Tag::StrList => "str_list",
            Tag::Null => "null",
            Tag::Record => "record",
            Tag::RecordList => "record_list",
        }
    }
}

/// A registry value. Adjacently tagged in serde so the CLI's JSON output is
/// self-describing (`{"type":"uint","value":4}`), matching the wire tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Str(String),
    Int(i64),
    Uint(u64),
    Bool(bool),
    Double(f64),
    Bytes(Vec<u8>),
    StrList(Vec<String>),
    /// Explicitly empty — **distinct from an absent key** (design §"Unset vs
    /// default"). `Unset` reverts to the schema default; `Null` is a value the
    /// user chose.
    Null,
    /// String-keyed, one level: every value must be a scalar.
    Record(BTreeMap<String, Value>),
    /// Ordered list of [`Value::Record`].
    RecordList(Vec<BTreeMap<String, Value>>),
}

impl Value {
    pub fn tag(&self) -> Tag {
        match self {
            Value::Str(_) => Tag::Str,
            Value::Int(_) => Tag::Int,
            Value::Uint(_) => Tag::Uint,
            Value::Bool(_) => Tag::Bool,
            Value::Double(_) => Tag::Double,
            Value::Bytes(_) => Tag::Bytes,
            Value::StrList(_) => Tag::StrList,
            Value::Null => Tag::Null,
            Value::Record(_) => Tag::Record,
            Value::RecordList(_) => Tag::RecordList,
        }
    }

    /// Is this a value a `Record` field may hold? Records are one level deep,
    /// so only the non-container tags qualify.
    pub fn is_scalar(&self) -> bool {
        !matches!(self, Value::Record(_) | Value::RecordList(_))
    }

    /// Bytes this value occupies when encoded — the daemon's size cap is
    /// enforced against this, not against a self-reported length.
    pub fn encoded_len(&self) -> usize {
        let mut buf = Vec::new();
        self.encode(&mut buf);
        buf.len()
    }

    /// Encode as `tag byte || payload`. Infallible: every `Value` is encodable
    /// by construction.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.tag().as_u8());
        match self {
            Value::Str(s) => put_bytes(out, s.as_bytes()),
            Value::Int(i) => out.extend_from_slice(&i.to_le_bytes()),
            Value::Uint(u) => out.extend_from_slice(&u.to_le_bytes()),
            Value::Bool(b) => out.push(u8::from(*b)),
            Value::Double(d) => out.extend_from_slice(&d.to_bits().to_le_bytes()),
            Value::Bytes(b) => put_bytes(out, b),
            Value::StrList(list) => {
                put_len(out, list.len());
                for s in list {
                    put_bytes(out, s.as_bytes());
                }
            }
            Value::Null => {}
            Value::Record(map) => {
                put_len(out, map.len());
                for (k, v) in map {
                    put_bytes(out, k.as_bytes());
                    v.encode(out);
                }
            }
            Value::RecordList(rows) => {
                put_len(out, rows.len());
                for row in rows {
                    put_len(out, row.len());
                    for (k, v) in row {
                        put_bytes(out, k.as_bytes());
                        v.encode(out);
                    }
                }
            }
        }
    }

    /// Decode bytes produced by [`Value::encode`]. Rejects trailing garbage, a
    /// non-scalar inside a record, and any length that would overrun the input
    /// — a corrupt row is an error, never a partial value.
    pub fn decode(input: &[u8]) -> Result<Value, DecodeError> {
        let mut cursor = Cursor {
            bytes: input,
            pos: 0,
        };
        let value = decode_value(&mut cursor)?;
        if cursor.pos != input.len() {
            return Err(DecodeError::TrailingBytes(input.len() - cursor.pos));
        }
        Ok(value)
    }
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u32).to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_len(out, bytes.len());
    out.extend_from_slice(bytes);
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        if end > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i64(&mut self) -> Result<i64, DecodeError> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes(b.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }

    fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        let raw = self.bytes()?;
        String::from_utf8(raw).map_err(|_| DecodeError::NotUtf8)
    }
}

fn decode_value(cursor: &mut Cursor<'_>) -> Result<Value, DecodeError> {
    let tag = Tag::from_u8(cursor.u8()?).ok_or(DecodeError::UnknownTag)?;
    Ok(match tag {
        Tag::Str => Value::Str(cursor.string()?),
        Tag::Int => Value::Int(cursor.i64()?),
        Tag::Uint => Value::Uint(cursor.u64()?),
        Tag::Bool => match cursor.u8()? {
            0 => Value::Bool(false),
            1 => Value::Bool(true),
            _ => return Err(DecodeError::BadBool),
        },
        Tag::Double => Value::Double(cursor.f64()?),
        Tag::Bytes => Value::Bytes(cursor.bytes()?),
        Tag::StrList => {
            let count = cursor.u32()? as usize;
            let mut list = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                list.push(cursor.string()?);
            }
            Value::StrList(list)
        }
        Tag::Null => Value::Null,
        Tag::Record => Value::Record(decode_record(cursor)?),
        Tag::RecordList => {
            let count = cursor.u32()? as usize;
            let mut rows = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                rows.push(decode_record(cursor)?);
            }
            Value::RecordList(rows)
        }
    })
}

fn decode_record(cursor: &mut Cursor<'_>) -> Result<BTreeMap<String, Value>, DecodeError> {
    let count = cursor.u32()? as usize;
    let mut map = BTreeMap::new();
    for _ in 0..count {
        let key = cursor.string()?;
        let value = decode_value(cursor)?;
        if !value.is_scalar() {
            return Err(DecodeError::NestedRecord);
        }
        map.insert(key, value);
    }
    Ok(map)
}

/// Why a byte string was not a valid encoded [`Value`]. Every variant means the
/// row is rejected outright — the daemon never surfaces a half-decoded value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    UnknownTag,
    BadBool,
    NotUtf8,
    NestedRecord,
    TrailingBytes(usize),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "value bytes ended early"),
            DecodeError::UnknownTag => write!(f, "unknown value tag"),
            DecodeError::BadBool => write!(f, "bool payload was not 0 or 1"),
            DecodeError::NotUtf8 => write!(f, "string payload was not UTF-8"),
            DecodeError::NestedRecord => {
                write!(
                    f,
                    "a record field held a nested record (records are one level)"
                )
            }
            DecodeError::TrailingBytes(n) => write!(f, "{n} trailing bytes after value"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: Value) {
        let mut buf = Vec::new();
        value.encode(&mut buf);
        let back = Value::decode(&buf).expect("decodes");
        assert_eq!(back, value, "round trip");
        assert_eq!(buf.len(), value.encoded_len());
    }

    #[test]
    fn every_tag_round_trips() {
        round_trip(Value::Str("hello".into()));
        round_trip(Value::Int(-7));
        round_trip(Value::Uint(7));
        round_trip(Value::Bool(true));
        round_trip(Value::Double(1.5));
        round_trip(Value::Bytes(vec![0, 1, 2, 255]));
        round_trip(Value::StrList(vec!["a".into(), "b".into()]));
        round_trip(Value::Null);
        round_trip(Value::Record(BTreeMap::from([
            ("name".to_string(), Value::Str("Web".into())),
            ("size".to_string(), Value::Uint(2)),
        ])));
        round_trip(Value::RecordList(vec![
            BTreeMap::from([("id".to_string(), Value::Str("firefox".into()))]),
            BTreeMap::from([("id".to_string(), Value::Str("chromium".into()))]),
        ]));
    }

    #[test]
    fn empty_containers_round_trip() {
        round_trip(Value::StrList(vec![]));
        round_trip(Value::Record(BTreeMap::new()));
        round_trip(Value::RecordList(vec![]));
    }

    #[test]
    fn null_is_not_absent_and_has_its_own_tag() {
        assert_eq!(Value::Null.tag(), Tag::Null);
        let mut buf = Vec::new();
        Value::Null.encode(&mut buf);
        assert_eq!(buf, vec![Tag::Null.as_u8()]);
    }

    #[test]
    fn truncated_input_is_rejected() {
        let mut buf = Vec::new();
        Value::Str("hello".into()).encode(&mut buf);
        buf.truncate(buf.len() - 1);
        assert_eq!(Value::decode(&buf), Err(DecodeError::Truncated));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut buf = Vec::new();
        Value::Bool(true).encode(&mut buf);
        buf.push(0);
        assert_eq!(Value::decode(&buf), Err(DecodeError::TrailingBytes(1)));
    }

    #[test]
    fn unknown_tag_is_rejected() {
        assert_eq!(Value::decode(&[200]), Err(DecodeError::UnknownTag));
    }

    #[test]
    fn bad_bool_payload_is_rejected() {
        assert_eq!(
            Value::decode(&[Tag::Bool.as_u8(), 2]),
            Err(DecodeError::BadBool)
        );
    }

    #[test]
    fn non_utf8_string_is_rejected() {
        let mut buf = vec![Tag::Str.as_u8()];
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&[0xff, 0xfe]);
        assert_eq!(Value::decode(&buf), Err(DecodeError::NotUtf8));
    }

    #[test]
    fn a_record_field_that_is_a_record_is_rejected() {
        // Hand-build `Record{ "nested": Record{} }` — the encoder cannot
        // produce it (records are one level), so craft the bytes directly.
        let mut inner = vec![Tag::Record.as_u8()];
        inner.extend_from_slice(&0u32.to_le_bytes());
        let mut buf = vec![Tag::Record.as_u8()];
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&6u32.to_le_bytes());
        buf.extend_from_slice(b"nested");
        buf.extend_from_slice(&inner);
        assert_eq!(Value::decode(&buf), Err(DecodeError::NestedRecord));
    }

    #[test]
    fn tag_bytes_are_stable() {
        // These are on-disk and on-wire values; changing one is a format break.
        assert_eq!(Tag::Str.as_u8(), 0);
        assert_eq!(Tag::Int.as_u8(), 1);
        assert_eq!(Tag::Uint.as_u8(), 2);
        assert_eq!(Tag::Bool.as_u8(), 3);
        assert_eq!(Tag::Double.as_u8(), 4);
        assert_eq!(Tag::Bytes.as_u8(), 5);
        assert_eq!(Tag::StrList.as_u8(), 6);
        assert_eq!(Tag::Null.as_u8(), 7);
        assert_eq!(Tag::Record.as_u8(), 8);
        assert_eq!(Tag::RecordList.as_u8(), 9);
    }

    #[test]
    fn scalars_are_not_containers() {
        assert!(Value::Str("x".into()).is_scalar());
        assert!(Value::StrList(vec![]).is_scalar());
        assert!(!Value::Record(BTreeMap::new()).is_scalar());
        assert!(!Value::RecordList(vec![]).is_scalar());
    }

    #[test]
    fn json_is_self_describing() {
        let json = serde_json::to_string(&Value::Uint(4)).unwrap();
        assert_eq!(json, r#"{"type":"uint","value":4}"#);
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Value::Uint(4));
        let null = serde_json::to_string(&Value::Null).unwrap();
        assert_eq!(null, r#"{"type":"null"}"#);
    }
}
