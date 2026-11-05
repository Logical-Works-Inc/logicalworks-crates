//! `json` owns JSON encoding and decoding for external interfaces.
//! Built on serde_json for correctness and interoperability. For
//! internal binary wire format, use the `wire` module (feature `wire`).
//!
//! INV-JSON-UTF8: all output is valid UTF-8. Input is validated as
//! UTF-8 during deserialization.
//!
//! [`Value`](crate::json::Value) is this crate's own tree, not `serde_json`'s: a string of up to
//! 22 bytes is stored inline and an object is one sorted
//! allocation, so a small document parses with one allocation per array or
//! object instead of one per key, string and tree node (#831). `serde_json`
//! writes the text and reads it into any other type; `text.parse::<Value>()`
//! reads the tree in one direct pass, handing `serde_json` every token and
//! every refusal it does not read plainly; `From` converts between the trees.

/// How a [`Value`] is built, compared, written, read and converted.
mod convert;
/// The object type.
mod map;
/// The direct reader behind `str::parse::<Value>()`.
mod read;
/// The inline-or-heap string.
mod text;
/// The value tree.
mod value;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use map::{IntoIter, Iter, Map};
pub use serde::{self, Deserialize, Serialize};
pub use serde_json::{Deserializer, Error, Number};
pub use text::Text;
pub use value::{Value, ValueIndex};

/// Builds a [`Value`] from JSON-shaped tokens, as `serde_json::json!` does:
/// `json!({"key": [1, "two", null], "flag": true})`. An interpolated
/// expression must implement [`Serialize`].
pub use crate::__lgwks_json_value as json;

/// The `serde_json` macro the [`json!`] macro expands to; not API.
#[doc(hidden)]
pub use serde_json::json as __serde_json_value;

/// The expansion behind [`json!`](crate::json::json): `serde_json` builds its
/// tree from the tokens and the tree converts into this crate's [`Value`].
#[doc(hidden)]
#[macro_export]
macro_rules! __lgwks_json_value {
    ($($json:tt)+) => {
        $crate::json::Value::from($crate::json::__serde_json_value!($($json)+))
    };
}

// ── Encoding ────────────────────────────────────────────────────────────────

/// Serialize `value` to a compact JSON string (no whitespace, no newlines).
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value)
}

/// Serialize `value` to a pretty-printed JSON string with indentation.
pub fn to_string_pretty<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    serde_json::to_string_pretty(value)
}

/// Serialize `value` to a byte vector (compact JSON).
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value)
}

/// Serialize `value` to a writer.
pub fn to_writer<W: std::io::Write, T: Serialize + ?Sized>(
    writer: W,
    value: &T,
) -> Result<(), Error> {
    serde_json::to_writer(writer, value)
}

// ── Decoding ────────────────────────────────────────────────────────────────

/// Deserialize JSON while allowing unescaped strings to borrow from `text`.
///
/// Escaped strings need decoded storage and therefore cannot deserialize into
/// a borrowed `&str`; use an owned `String` field for inputs that may escape.
pub fn from_str<'de, T: serde::Deserialize<'de>>(text: &'de str) -> Result<T, Error> {
    serde_json::from_str(text)
}

/// Deserialize JSON while allowing unescaped strings to borrow from `bytes`.
///
/// Escaped strings need decoded storage and therefore cannot deserialize into
/// a borrowed `&str`; use an owned `String` field for inputs that may escape.
pub fn from_slice<'de, T: serde::Deserialize<'de>>(bytes: &'de [u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes)
}

/// Deserialize the entire `reader` as JSON into `T`. The reader is consumed to
/// EOF; an I/O failure and a syntax failure both surface as [`Error`].
pub fn from_reader<R: std::io::Read, T: serde::de::DeserializeOwned>(
    reader: R,
) -> Result<T, Error> {
    serde_json::from_reader(reader)
}

/// Deserialize a [`Value`] into the requested type.
pub fn from_value<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(serde_json::Value::from(value))
}

/// Build a [`Value`] tree from `value`. Fails when `value` contains a map whose
/// keys are not strings, or when its `Serialize` impl reports an error.
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, Error> {
    serde_json::to_value(value).map(Value::from)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct Point {
        x: i32,
        y: i32,
    }

    /// The round-trip fixture the codec's own tests render, shape and parse.
    const POINT: Point = Point { x: 1, y: 2 };

    // These tests return `Result` rather than unwrapping: a JSON refusal
    // reports its own `Debug` on failure, which is the same report `.unwrap`
    // would have panicked with, without an `unwrap` in the tree.
    #[test]
    fn roundtrips_through_string() -> Result<(), Error> {
        let text = to_string(&POINT)?;
        let restored: Point = from_str(&text)?;
        assert_eq!(POINT, restored);
        Ok(())
    }

    #[test]
    fn compact_output_has_no_whitespace() -> Result<(), Error> {
        let text = to_string(&POINT)?;
        assert!(!text.contains('\n'));
        assert!(!text.contains("  "));
        Ok(())
    }

    #[test]
    fn pretty_output_has_indentation() -> Result<(), Error> {
        assert!(
            to_string_pretty(&POINT)?.contains('\n'),
            "pretty JSON spans lines"
        );
        Ok(())
    }

    #[test]
    fn from_str_rejects_invalid() {
        assert!(
            from_str::<Point>("{").is_err(),
            "a truncated document is refused"
        );
    }

    /// INV-CODEC-1 for this codec: an unescaped field borrows from the input,
    /// through both the text and the slice entry point.
    #[test]
    fn unescaped_string_fields_borrow_from_text_and_slice() -> Result<(), Box<dyn std::error::Error>>
    {
        crate::serde_facade::json_borrow_contract()?;
        Ok(())
    }

    /// The other half of INV-CODEC-1 for this codec: an escaped field cannot
    /// come back as a borrow, because it needs owned decoded storage.
    #[test]
    fn escaped_string_cannot_be_returned_as_a_borrowed_str() {
        crate::serde_facade::json_escaped_field_is_refused();
    }

    #[test]
    fn borrowed_serializers_accept_str_and_slices() -> Result<(), Error> {
        let text: &str = "plain";
        assert_eq!(to_string(text)?, "\"plain\"", "str serializes directly");
        assert_eq!(
            to_string_pretty(text)?,
            "\"plain\"",
            "pretty serialization accepts str"
        );
        assert_eq!(
            to_vec([1, 2].as_slice())?,
            b"[1,2]",
            "slices serialize directly"
        );
        let mut output = Vec::new();
        to_writer(&mut output, [1, 2].as_slice())?;
        assert_eq!(output, b"[1,2]", "writer serialization accepts slices");
        assert_eq!(
            to_value([1, 2].as_slice())?,
            Value::Array(vec![Value::from(1), Value::from(2)]),
            "value-tree serialization accepts slices"
        );
        Ok(())
    }

    #[test]
    fn parse_errors_keep_utf8_location_and_trailing_content_details() {
        let invalid_utf8: Result<Point, _> = from_slice(&[0xFF]);
        assert!(
            matches!(invalid_utf8, Err(error) if error.classify() == serde_json::error::Category::Syntax),
            "invalid UTF-8 is a syntax failure through the facade"
        );

        let malformed: Result<Point, _> = from_str("{\n  \"x\": }");
        assert!(
            malformed
                .as_ref()
                .err()
                .is_some_and(|error| error.line() == 2 && error.column() > 0),
            "syntax errors retain their source line and column"
        );

        let trailing: Result<Point, _> = from_str(r#"{"x":1,"y":2} trailing"#);
        assert!(
            matches!(trailing, Err(error) if error.classify() == serde_json::error::Category::Syntax),
            "non-whitespace trailing content remains a syntax error"
        );
    }
}
