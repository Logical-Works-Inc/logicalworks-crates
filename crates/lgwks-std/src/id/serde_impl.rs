//! Serde for [`Uuid`] (#851): the hyphenated 8-4-4-4-12 text in a
//! human-readable format and the 16 network-order bytes otherwise, the shape
//! the `uuid` crate writes, so a document it persisted reads back unchanged.
//!
//! INV-ID-3: text in, text out for a human-readable format; exactly 16 bytes,
//! or a sequence of exactly 16 bytes, for any other. Every other length and
//! every malformed text is refused, never padded or truncated.

use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Uuid;

/// The width of the raw form, in bytes.
const RAW_LEN: usize = 16;

impl Serialize for Uuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            serializer.serialize_bytes(self.as_bytes())
        }
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_str(UuidVisitor)
        } else {
            deserializer.deserialize_bytes(UuidVisitor)
        }
    }
}

/// Accepts the canonical text, 16 raw bytes, or a sequence of 16 bytes.
struct UuidVisitor;

impl<'de> Visitor<'de> for UuidVisitor {
    type Value = Uuid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a hyphenated UUID string or 16 bytes")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Uuid, E> {
        Uuid::parse(text).map_err(E::custom)
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Uuid, E> {
        match <[u8; RAW_LEN]>::try_from(bytes) {
            Ok(raw) => Ok(Uuid::from(raw)),
            Err(_wrong_width) => Err(E::invalid_length(bytes.len(), &self)),
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Uuid, A::Error> {
        let mut raw = [0u8; RAW_LEN];
        for (index, slot) in raw.iter_mut().enumerate() {
            *slot = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(index, &self))?;
        }
        match seq.next_element::<u8>()? {
            None => Ok(Uuid::from(raw)),
            Some(_extra) => Err(de::Error::invalid_length(RAW_LEN.saturating_add(1), &self)),
        }
    }
}
