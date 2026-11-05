//! How a [`Value`] is built from Rust values, compared with them, written and
//! read through serde, and exchanged with `serde_json`'s own tree (#831).

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};

use super::{Map, Number, Text, Value};

/// The most items or members reserved up front from a declared length, so a
/// hostile length hint cannot reserve memory the input never fills.
const RESERVE_CEILING: usize = 4_096;

/// The members an object reserves before its first, so the common object of
/// up to eight members is read into one allocation instead of growing twice.
const FIRST_MEMBERS: usize = 8;

/// What to reserve for a container whose reader declared `hint` entries: the
/// hint up to [`RESERVE_CEILING`], and nothing for an undeclared length.
fn reserve(hint: Option<usize>) -> usize {
    match hint {
        Some(declared) => declared.min(RESERVE_CEILING),
        None => 0,
    }
}

// ── Building ────────────────────────────────────────────────────────────────

impl From<bool> for Value {
    fn from(flag: bool) -> Self {
        Self::Bool(flag)
    }
}

impl From<Number> for Value {
    fn from(number: Number) -> Self {
        Self::Number(number)
    }
}

/// Integers become numbers exactly.
macro_rules! from_integer {
    ($($integer:ty),*) => {$(
        impl From<$integer> for Value {
            fn from(integer: $integer) -> Self {
                Self::Number(Number::from(integer))
            }
        }
    )*};
}

from_integer!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<f64> for Value {
    /// A finite float is a number; NaN and the infinities have no JSON form
    /// and are `null`, as `serde_json` writes them.
    fn from(float: f64) -> Self {
        match Number::from_f64(float) {
            Some(number) => Self::Number(number),
            None => Self::Null,
        }
    }
}

impl From<f32> for Value {
    /// As `f64`.
    fn from(float: f32) -> Self {
        Self::from(f64::from(float))
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::String(Text::from(text))
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::String(Text::from(text))
    }
}

impl From<&String> for Value {
    fn from(text: &String) -> Self {
        Self::String(Text::from(text.as_str()))
    }
}

impl From<Text> for Value {
    fn from(text: Text) -> Self {
        Self::String(text)
    }
}

impl From<Map> for Value {
    fn from(members: Map) -> Self {
        Self::Object(members)
    }
}

impl From<()> for Value {
    fn from((): ()) -> Self {
        Self::Null
    }
}

impl<T: Into<Self>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Self::Array(items.into_iter().map(Into::into).collect())
    }
}

impl<T: Clone + Into<Self>> From<&[T]> for Value {
    fn from(items: &[T]) -> Self {
        Self::Array(items.iter().cloned().map(Into::into).collect())
    }
}

impl<T: Into<Self>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::Null,
        }
    }
}

impl<T: Into<Self>> FromIterator<T> for Value {
    /// An array of the items.
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        Self::Array(items.into_iter().map(Into::into).collect())
    }
}

// ── Comparing ───────────────────────────────────────────────────────────────

impl PartialEq<str> for Value {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for Value {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<String> for Value {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}

impl PartialEq<bool> for Value {
    fn eq(&self, other: &bool) -> bool {
        self.as_bool() == Some(*other)
    }
}

impl PartialEq<f64> for Value {
    /// Equal when the value is a number whose `f64` reading is bit-identical.
    fn eq(&self, other: &f64) -> bool {
        matches!(self.as_f64(), Some(number) if number.to_bits() == other.to_bits())
    }
}

/// A signed integer equals a number that reads back as the same `i64`.
macro_rules! eq_signed {
    ($($integer:ty),*) => {$(
        impl PartialEq<$integer> for Value {
            fn eq(&self, other: &$integer) -> bool {
                self.as_i64() == Some(i64::from(*other))
            }
        }
    )*};
}

eq_signed!(i8, i16, i32, i64);

/// An unsigned integer equals a number that reads back as the same `u64`.
macro_rules! eq_unsigned {
    ($($integer:ty),*) => {$(
        impl PartialEq<$integer> for Value {
            fn eq(&self, other: &$integer) -> bool {
                self.as_u64() == Some(u64::from(*other))
            }
        }
    )*};
}

eq_unsigned!(u8, u16, u32, u64);

impl PartialEq<usize> for Value {
    fn eq(&self, other: &usize) -> bool {
        match u64::try_from(*other) {
            Ok(wide) => self.as_u64() == Some(wide),
            Err(_) => false,
        }
    }
}

// ── Writing ─────────────────────────────────────────────────────────────────

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(flag) => serializer.serialize_bool(flag),
            Self::Number(ref number) => number.serialize(serializer),
            Self::String(ref text) => serializer.serialize_str(text.as_str()),
            Self::Array(ref items) => {
                let mut sequence = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    sequence.serialize_element(item)?;
                }
                sequence.end()
            }
            Self::Object(ref members) => members.serialize(serializer),
        }
    }
}

// ── Reading ─────────────────────────────────────────────────────────────────

/// Reads any JSON value, copying every string of up to 22 bytes inline.
struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, flag: bool) -> Result<Value, E> {
        Ok(Value::Bool(flag))
    }

    fn visit_i64<E: de::Error>(self, integer: i64) -> Result<Value, E> {
        Ok(Value::Number(Number::from(integer)))
    }

    fn visit_u64<E: de::Error>(self, integer: u64) -> Result<Value, E> {
        Ok(Value::Number(Number::from(integer)))
    }

    fn visit_f64<E: de::Error>(self, float: f64) -> Result<Value, E> {
        Ok(Value::from(float))
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Value, E> {
        Ok(Value::String(Text::from(text)))
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Value, E> {
        Ok(Value::String(Text::from(text)))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Value, A::Error> {
        let mut array = Vec::with_capacity(reserve(items.size_hint()));
        while let Some(item) = items.next_element()? {
            array.push(item);
        }
        Ok(Value::Array(array))
    }

    fn visit_map<A: MapAccess<'de>>(self, members: A) -> Result<Value, A::Error> {
        Ok(Value::Object(read_members(members)?))
    }
}

/// Reads an object's members in document order and sorts them once.
fn read_members<'de, A: MapAccess<'de>>(mut members: A) -> Result<Map, A::Error> {
    let mut entries: Vec<(Text, Value)> =
        Vec::with_capacity(reserve(members.size_hint()).max(FIRST_MEMBERS));
    while let Some(key) = members.next_key::<Text>()? {
        let value = members.next_value::<Value>()?;
        entries.push((key, value));
    }
    Ok(Map::from_members(entries))
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// Reads a JSON object into a [`Map`].
struct MapVisitor;

impl<'de> Visitor<'de> for MapVisitor {
    type Value = Map;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, members: A) -> Result<Map, A::Error> {
        read_members(members)
    }
}

impl<'de> Deserialize<'de> for Map {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(MapVisitor)
    }
}

// ── serde_json's tree ───────────────────────────────────────────────────────

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(flag) => Self::Bool(flag),
            serde_json::Value::Number(number) => Self::Number(number),
            serde_json::Value::String(text) => Self::String(Text::from(text)),
            serde_json::Value::Array(items) => {
                Self::Array(items.into_iter().map(Self::from).collect())
            }
            serde_json::Value::Object(members) => Self::Object(Map::from_members(
                members
                    .into_iter()
                    .map(|(key, member)| (Text::from(key), Self::from(member)))
                    .collect(),
            )),
        }
    }
}

impl From<Value> for serde_json::Value {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(flag) => Self::Bool(flag),
            Value::Number(number) => Self::Number(number),
            Value::String(text) => Self::String(String::from(text)),
            Value::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            Value::Object(members) => Self::Object(
                members
                    .into_iter()
                    .map(|(key, member)| (key, Self::from(member)))
                    .collect(),
            ),
        }
    }
}
