//! [`Value`]: any JSON document, owned, with no allocation for a string of up
//! to [`INLINE_CAPACITY`](super::INLINE_CAPACITY) bytes and one allocation per
//! array or object (#831).

use std::fmt;
use std::ops::Index;

use super::{Map, Number, Text};

/// The value an absent key or index reads as.
pub(super) static NULL: Value = Value::Null;

/// Any JSON value, per RFC 8259 §3.
#[non_exhaustive]
#[derive(Clone, Default, PartialEq, Eq)]
pub enum Value {
    /// `null`.
    #[default]
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as `serde_json` reads and writes it.
    Number(Number),
    /// A string, inline up to 22 bytes.
    String(Text),
    /// An array.
    Array(Vec<Value>),
    /// An object, its keys sorted.
    Object(Map),
}

impl Value {
    /// Whether the value is `null`.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(*self, Self::Null)
    }

    /// Whether the value is a string.
    #[must_use]
    pub const fn is_string(&self) -> bool {
        matches!(*self, Self::String(_))
    }

    /// The boolean, if the value is one.
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(flag) => Some(flag),
            Self::Null | Self::Number(_) | Self::String(_) | Self::Array(_) | Self::Object(_) => {
                None
            }
        }
    }

    /// The number, if the value is one.
    const fn as_number(&self) -> Option<&Number> {
        match *self {
            Self::Number(ref number) => Some(number),
            Self::Null | Self::Bool(_) | Self::String(_) | Self::Array(_) | Self::Object(_) => None,
        }
    }

    /// The number as `u64`, if it is a non-negative integer that fits.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(Number::as_u64)
    }

    /// The number as `i64`, if it is an integer that fits.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }

    /// The number as `f64`, if the value is a number.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().and_then(Number::as_f64)
    }

    /// The string, if the value is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match *self {
            Self::String(ref text) => Some(text.as_str()),
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::Array(_) | Self::Object(_) => None,
        }
    }

    /// The array, if the value is one.
    #[must_use]
    pub const fn as_array(&self) -> Option<&Vec<Self>> {
        match *self {
            Self::Array(ref items) => Some(items),
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) | Self::Object(_) => {
                None
            }
        }
    }

    /// The items to push into, remove or rewrite in place; `None` for any
    /// other kind.
    #[must_use]
    pub const fn as_array_mut(&mut self) -> Option<&mut Vec<Self>> {
        match *self {
            Self::Array(ref mut items) => Some(items),
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) | Self::Object(_) => {
                None
            }
        }
    }

    /// The object, if the value is one.
    #[must_use]
    pub const fn as_object(&self) -> Option<&Map> {
        match *self {
            Self::Object(ref members) => Some(members),
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) | Self::Array(_) => None,
        }
    }

    /// The members to insert into, remove or rewrite in place; `None` for
    /// any other kind.
    #[must_use]
    pub const fn as_object_mut(&mut self) -> Option<&mut Map> {
        match *self {
            Self::Object(ref mut members) => Some(members),
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) | Self::Array(_) => None,
        }
    }

    /// The member under a key of an object, or the item at an index of an
    /// array; `None` for an absent one or a value of another kind.
    #[must_use]
    pub fn get<I: ValueIndex>(&self, index: I) -> Option<&Self> {
        index.index_into(self)
    }

    /// The member or item, mutably.
    #[must_use]
    pub fn get_mut<I: ValueIndex>(&mut self, index: I) -> Option<&mut Self> {
        index.index_into_mut(self)
    }
}

/// What [`Value::get`] and `value[..]` accept: a key for an object, an index
/// for an array.
pub trait ValueIndex: sealed::Sealed {
    /// The member or item this index names in `value`.
    fn index_into<'value>(&self, value: &'value Value) -> Option<&'value Value>;

    /// The member or item, mutably.
    fn index_into_mut<'value>(&self, value: &'value mut Value) -> Option<&'value mut Value>;
}

/// Keeps [`ValueIndex`] to the key and index types this module names.
mod sealed {
    /// The seal.
    pub trait Sealed {}
    impl Sealed for usize {}
    impl Sealed for str {}
    impl Sealed for String {}
    impl<T: Sealed + ?Sized> Sealed for &T {}
}

impl ValueIndex for usize {
    fn index_into<'value>(&self, value: &'value Value) -> Option<&'value Value> {
        value.as_array()?.get(*self)
    }

    fn index_into_mut<'value>(&self, value: &'value mut Value) -> Option<&'value mut Value> {
        value.as_array_mut()?.get_mut(*self)
    }
}

impl ValueIndex for str {
    fn index_into<'value>(&self, value: &'value Value) -> Option<&'value Value> {
        value.as_object()?.get(self)
    }

    fn index_into_mut<'value>(&self, value: &'value mut Value) -> Option<&'value mut Value> {
        value.as_object_mut()?.get_mut(self)
    }
}

impl ValueIndex for String {
    fn index_into<'value>(&self, value: &'value Value) -> Option<&'value Value> {
        self.as_str().index_into(value)
    }

    fn index_into_mut<'value>(&self, value: &'value mut Value) -> Option<&'value mut Value> {
        self.as_str().index_into_mut(value)
    }
}

impl<T: ValueIndex + ?Sized> ValueIndex for &T {
    fn index_into<'value>(&self, value: &'value Value) -> Option<&'value Value> {
        (**self).index_into(value)
    }

    fn index_into_mut<'value>(&self, value: &'value mut Value) -> Option<&'value mut Value> {
        (**self).index_into_mut(value)
    }
}

impl<I: ValueIndex> Index<I> for Value {
    type Output = Self;

    /// The member or item, or `null` when it is absent or the value is of
    /// another kind; reading never panics.
    fn index(&self, index: I) -> &Self {
        match index.index_into(self) {
            Some(value) => value,
            None => &NULL,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Null => formatter.write_str("Null"),
            Self::Bool(flag) => write!(formatter, "Bool({flag})"),
            Self::Number(ref number) => write!(formatter, "Number({number})"),
            Self::String(ref text) => write!(formatter, "String({text:?})"),
            Self::Array(ref items) => {
                formatter.write_str("Array ")?;
                formatter.debug_list().entries(items).finish()
            }
            Self::Object(ref members) => {
                formatter.write_str("Object ")?;
                fmt::Debug::fmt(members, formatter)
            }
        }
    }
}

impl fmt::Display for Value {
    /// Compact JSON, or indented JSON under `{:#}`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rendered = if formatter.alternate() {
            super::to_string_pretty(self)
        } else {
            super::to_string(self)
        };
        match rendered {
            Ok(text) => formatter.write_str(&text),
            Err(_) => Err(fmt::Error),
        }
    }
}

impl std::str::FromStr for Value {
    type Err = super::Error;

    /// Reads the tree in one direct pass; text that pass leaves alone is read
    /// by `serde_json`, so the value and any refusal are `serde_json`'s.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match super::read::document(text) {
            Some(value) => Ok(value),
            None => super::from_str(text),
        }
    }
}
