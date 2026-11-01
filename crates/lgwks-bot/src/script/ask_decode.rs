//! Decoding model output into the declared type.
//!
//! [`AskDecode`] is the one door an `ask` decodes through: the declared type
//! at the binding decides how the bytes read, and a refusal is typed —
//! [`FlowError::Failed`] naming the offset where text or structure stopped —
//! never a silent prefix or a guessed value. [`Vec<u8>`] takes the bytes
//! whole, [`String`] takes them as UTF-8, and [`Json`] takes them as a JSON
//! document of `T`.

use super::FlowError;

/// Decode model output into the declared type.
///
/// The trait a binding's type implements to be askable for. Truncation is
/// refused before this is called, so every implementation reads whole bytes
/// and refuses what it cannot read whole.
///
/// # Errors
///
/// [`FlowError::Failed`] when the bytes are not this type's value.
pub trait AskDecode: Sized {
    /// Decode whole output bytes into this type.
    fn decode(bytes: &[u8]) -> Result<Self, FlowError>;
}

impl AskDecode for Vec<u8> {
    /// The bytes, whole.
    fn decode(bytes: &[u8]) -> Result<Self, FlowError> {
        Ok(bytes.to_vec())
    }
}

impl AskDecode for String {
    /// The bytes as UTF-8, refused at the first invalid offset.
    fn decode(bytes: &[u8]) -> Result<Self, FlowError> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(text.to_owned()),
            Err(invalid) => {
                let refusal = Err(FlowError::failed(format!(
                    "the model output is not UTF-8 at byte {}",
                    invalid.valid_up_to()
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode: the output is not text");
                refusal
            }
        }
    }
}

/// A JSON document of `T`, decoded from model output.
///
/// The wrapper a binding names when the declared type is JSON: `let reply:
/// Json<Report> = ask model with (request)?` decodes the output as one JSON
/// document of `Report` and refuses anything else — trailing bytes included —
/// rather than decoding a prefix and calling it the value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Json<T> {
    /// The decoded document.
    pub value: T,
}

impl<T> AskDecode for Json<T>
where
    T: lgwks_std::json::serde::de::DeserializeOwned,
{
    /// Decode the whole output as one JSON document of `T`.
    fn decode(bytes: &[u8]) -> Result<Self, FlowError> {
        match lgwks_std::json::from_slice::<T>(bytes) {
            Ok(value) => Ok(Self { value }),
            Err(refused) => {
                let refusal = Err(FlowError::failed(format!(
                    "the model output is not JSON: {refused}"
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode: the output is not JSON");
                refusal
            }
        }
    }
}
