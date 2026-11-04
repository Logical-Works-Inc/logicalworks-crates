//! Seeded replay of the serde shape of [`Uuid`] (INV-ID-3, #851).
//!
//! The text arm is held to RFC 9562 Appendix A's printed forms and to a
//! seeded round trip through both serde facades, JSON and RON. The binary arm
//! runs through a test serializer and deserializer that report themselves as
//! not human-readable, the switch a binary format such as bincode or postcard
//! throws, so the 16 raw bytes and the 16-element sequence are both read.
//! Every seed folds what it saw into a trace, and the same seed must give the
//! same trace.

use std::error::Error;
use std::fmt;

use serde::de::value::{Error as ValueError, SeqDeserializer};
use serde::de::{Deserializer, Visitor};
use serde::ser::{self, Impossible, Serializer};
use serde::{Deserialize, Serialize};

use super::{ParseError, Uuid};
use crate::seeded::Seeded;
use crate::{json, ron};

/// The seeds the sweeps replay: the estate's one named table.
const SEEDS: [u64; 4] = crate::seeded::SWEEP_SEEDS;

/// Identifiers drawn per seed and per arm.
const PER_SEED: usize = 4_096;

/// The mixing constant of the trace fold: the 64-bit FNV-1a prime.
const FNV_PRIME: u64 = 1_099_511_628_211;

/// The trace's starting value: the 64-bit FNV-1a offset basis.
const FNV_BASIS: u64 = 14_695_981_039_346_656_037;

/// RFC 9562 Appendix A.2, the version 4 example, as printed.
const RFC_V4: &str = "919108f7-52d1-4320-9bac-f847db4148a8";

/// RFC 9562 Appendix A.6, the version 7 example, as printed (upper case).
const RFC_V7: &str = "017F22E2-79B0-7CC3-98C4-DC0C0C07398F";

/// Why a probe of the serde shape could not run to its assertions.
#[derive(Debug)]
enum ShapeError {
    /// The JSON facade refused.
    Json(json::Error),
    /// The RON encoder refused.
    RonEncode(ron::Error),
    /// The RON decoder refused.
    RonDecode(ron::SpannedError),
    /// A fixture's text did not parse.
    Parse(ParseError),
    /// The binary test format refused.
    Binary(ValueError),
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Json(ref cause) => write!(formatter, "json refused: {cause}"),
            Self::RonEncode(ref cause) => write!(formatter, "ron encode refused: {cause}"),
            Self::RonDecode(ref cause) => write!(formatter, "ron decode refused: {cause}"),
            Self::Parse(ref cause) => write!(formatter, "fixture did not parse: {cause}"),
            Self::Binary(ref cause) => write!(formatter, "binary format refused: {cause}"),
        }
    }
}

impl Error for ShapeError {}

impl From<json::Error> for ShapeError {
    fn from(cause: json::Error) -> Self {
        Self::Json(cause)
    }
}

impl From<ron::Error> for ShapeError {
    fn from(cause: ron::Error) -> Self {
        Self::RonEncode(cause)
    }
}

impl From<ron::SpannedError> for ShapeError {
    fn from(cause: ron::SpannedError) -> Self {
        Self::RonDecode(cause)
    }
}

impl From<ParseError> for ShapeError {
    fn from(cause: ParseError) -> Self {
        Self::Parse(cause)
    }
}

impl From<ValueError> for ShapeError {
    fn from(cause: ValueError) -> Self {
        Self::Binary(cause)
    }
}

/// What every probe returns.
type ShapeResult<T> = Result<T, ShapeError>;

/// Folds every byte of `bytes` into the running trace.
fn fold_bytes(trace: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *trace = trace.wrapping_mul(FNV_PRIME).wrapping_add(u64::from(*byte));
    }
}

/// One identifier of 128 drawn bits, no version imposed.
fn draw_uuid(stream: &mut Seeded) -> Uuid {
    let [h0, h1, h2, h3, h4, h5, h6, h7] = stream.next_u64().to_be_bytes();
    let [l0, l1, l2, l3, l4, l5, l6, l7] = stream.next_u64().to_be_bytes();
    Uuid::from([
        h0, h1, h2, h3, h4, h5, h6, h7, l0, l1, l2, l3, l4, l5, l6, l7,
    ])
}

/// A non-human-readable input: the raw form, or the same bytes as a sequence.
enum Binary<'input> {
    /// A byte string, as `serialize_bytes` writes it.
    Bytes(&'input [u8]),
    /// A sequence of `u8`, as a format with no byte-string type writes it.
    Seq(&'input [u8]),
}

impl<'de> Deserializer<'de> for Binary<'_> {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ValueError> {
        match self {
            Self::Bytes(bytes) => visitor.visit_bytes(bytes),
            Self::Seq(bytes) => {
                visitor.visit_seq(SeqDeserializer::<_, ValueError>::new(bytes.iter().copied()))
            }
        }
    }

    fn is_human_readable(&self) -> bool {
        false
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

/// A non-human-readable output that keeps a byte string and refuses the rest.
struct Capture;

/// Writes `Capture` methods that refuse, naming the method they refused in.
macro_rules! refuse {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(fn $name(self, $($arg: $ty),*) -> Result<$ret, ValueError> {
            Err(ser::Error::custom(concat!(stringify!($name), " is not captured")))
        })*
    };
}

/// The compound serializer `Capture` never hands out.
type Never = Impossible<Vec<u8>, ValueError>;

impl Serializer for Capture {
    type Ok = Vec<u8>;
    type Error = ValueError;
    type SerializeSeq = Never;
    type SerializeTuple = Never;
    type SerializeTupleStruct = Never;
    type SerializeTupleVariant = Never;
    type SerializeMap = Never;
    type SerializeStruct = Never;
    type SerializeStructVariant = Never;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn serialize_bytes(self, bytes: &[u8]) -> Result<Vec<u8>, ValueError> {
        Ok(bytes.to_vec())
    }

    refuse! {
        serialize_bool(_value: bool) -> Vec<u8>;
        serialize_i8(_value: i8) -> Vec<u8>;
        serialize_i16(_value: i16) -> Vec<u8>;
        serialize_i32(_value: i32) -> Vec<u8>;
        serialize_i64(_value: i64) -> Vec<u8>;
        serialize_u8(_value: u8) -> Vec<u8>;
        serialize_u16(_value: u16) -> Vec<u8>;
        serialize_u32(_value: u32) -> Vec<u8>;
        serialize_u64(_value: u64) -> Vec<u8>;
        serialize_f32(_value: f32) -> Vec<u8>;
        serialize_f64(_value: f64) -> Vec<u8>;
        serialize_char(_value: char) -> Vec<u8>;
        serialize_str(_value: &str) -> Vec<u8>;
        serialize_none() -> Vec<u8>;
        serialize_unit() -> Vec<u8>;
        serialize_unit_struct(_name: &'static str) -> Vec<u8>;
        serialize_unit_variant(_name: &'static str, _index: u32, _variant: &'static str) -> Vec<u8>;
        serialize_seq(_len: Option<usize>) -> Never;
        serialize_tuple(_len: usize) -> Never;
        serialize_tuple_struct(_name: &'static str, _len: usize) -> Never;
        serialize_tuple_variant(_name: &'static str, _index: u32, _variant: &'static str, _len: usize) -> Never;
        serialize_map(_len: Option<usize>) -> Never;
        serialize_struct(_name: &'static str, _len: usize) -> Never;
        serialize_struct_variant(_name: &'static str, _index: u32, _variant: &'static str, _len: usize) -> Never;
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> Result<Vec<u8>, ValueError> {
        Err(ser::Error::custom("serialize_some is not captured"))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Vec<u8>, ValueError> {
        Err(ser::Error::custom(
            "serialize_newtype_struct is not captured",
        ))
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Vec<u8>, ValueError> {
        Err(ser::Error::custom(
            "serialize_newtype_variant is not captured",
        ))
    }
}

/// Every identifier `seed` draws, through JSON, RON and both binary inputs,
/// two tenants per document; returns the trace of every rendered form.
fn replay(seed: u64) -> ShapeResult<u64> {
    let mut stream = Seeded::from_seed(seed);
    let mut trace = FNV_BASIS;
    for _ in 0..PER_SEED {
        let tenants = (draw_uuid(&mut stream), draw_uuid(&mut stream));
        let text = json::to_string(&tenants)?;
        assert_eq!(
            text,
            format!("[\"{}\",\"{}\"]", tenants.0, tenants.1),
            "seed {seed}"
        );
        assert_eq!(
            json::from_str::<(Uuid, Uuid)>(&text)?,
            tenants,
            "seed {seed}"
        );
        let notation = ron::to_string(&tenants)?;
        assert_eq!(
            ron::from_str::<(Uuid, Uuid)>(&notation)?,
            tenants,
            "seed {seed}"
        );
        let raw = tenants.0.serialize(Capture)?;
        assert_eq!(raw.as_slice(), tenants.0.as_bytes(), "seed {seed}");
        assert_eq!(
            Uuid::deserialize(Binary::Bytes(&raw))?,
            tenants.0,
            "seed {seed}"
        );
        assert_eq!(
            Uuid::deserialize(Binary::Seq(&raw))?,
            tenants.0,
            "seed {seed}"
        );
        fold_bytes(&mut trace, text.as_bytes());
        fold_bytes(&mut trace, notation.as_bytes());
        fold_bytes(&mut trace, &raw);
    }
    Ok(trace)
}

#[test]
fn the_rfc_vectors_render_as_their_printed_lowercase_text() -> ShapeResult<()> {
    let v4 = Uuid::parse(RFC_V4)?;
    assert_eq!(json::to_string(&v4)?, format!("\"{RFC_V4}\""));
    let v7 = Uuid::parse(RFC_V7)?;
    let lower = RFC_V7.to_ascii_lowercase();
    assert_eq!(json::to_string(&v7)?, format!("\"{lower}\""));
    assert_eq!(json::from_str::<Uuid>(&format!("\"{RFC_V7}\""))?, v7);
    assert_eq!(ron::to_string(&v4)?, format!("\"{RFC_V4}\""));
    Ok(())
}

#[test]
fn every_seed_round_trips_through_text_bytes_and_sequences() -> ShapeResult<()> {
    for seed in SEEDS {
        replay(seed)?;
    }
    Ok(())
}

#[test]
fn the_same_seed_replays_to_the_same_trace_and_seeds_diverge() -> ShapeResult<()> {
    let [first, second, ..] = SEEDS;
    assert_eq!(replay(first)?, replay(first)?);
    assert_ne!(replay(first)?, replay(second)?);
    Ok(())
}

#[test]
fn a_wrong_width_byte_string_or_sequence_is_refused() -> ShapeResult<()> {
    let raw = Uuid::parse(RFC_V4)?.serialize(Capture)?;
    for width in [0, 15, 17] {
        let bytes: Vec<u8> = raw.iter().copied().cycle().take(width).collect();
        assert!(
            Uuid::deserialize(Binary::Bytes(&bytes)).is_err(),
            "bytes {width}"
        );
        assert!(
            Uuid::deserialize(Binary::Seq(&bytes)).is_err(),
            "seq {width}"
        );
    }
    Ok(())
}

#[test]
fn malformed_text_and_the_wrong_type_are_refused() {
    for document in [
        "\"919108f7-52d1-4320-9bac-f847db4148a\"",
        "\"919108f7-52d1-4320-9bac-f847db4148a8a\"",
        "\"919108g7-52d1-4320-9bac-f847db4148a8\"",
        "\"919108f7+52d1-4320-9bac-f847db4148a8\"",
        "\"\"",
        "17",
        "null",
        "[145,145,8,247,82,209,67,32,155,172,248,71,219,65,72,168]",
    ] {
        assert!(json::from_str::<Uuid>(document).is_err(), "{document}");
    }
}
