//! The FoundationDB tuple-layer element for [`Uuid`] (#851, INV-ID-5): type
//! code `0x30` followed by the 16 bytes in network order, the encoding the
//! tuple layer's specification fixes (`design/tuple.md` in apple/foundationdb,
//! "UUID: typecode 0x30, length 16 bytes, network byte order") and the one
//! `foundationdb-tuple` writes for `uuid::Uuid`. Network order makes the packed
//! bytes sort as the identifiers do, so a key that leads with a version 7
//! identifier sorts in issue order in the store.

use std::error::Error;
use std::fmt;

use super::Uuid;

/// The tuple layer's type code for a UUID element.
pub(super) const UUID_CODE: u8 = 0x30;

/// The width of a packed element: the code and the 16 bytes.
pub(super) const PACKED_LEN: usize = 17;

impl Uuid {
    /// Appends this identifier to `key` as one tuple-layer element: `0x30`
    /// and the 16 bytes in network order.
    pub fn pack_tuple(&self, key: &mut Vec<u8>) {
        key.push(UUID_CODE);
        key.extend_from_slice(self.as_bytes());
    }

    /// Reads one tuple-layer UUID element from the front of `key` and returns
    /// it with the bytes after it, which belong to the caller.
    ///
    /// # Errors
    ///
    /// [`TupleError::WrongCode`] when the first byte is not `0x30`, naming it,
    /// and [`TupleError::Truncated`] when the key ends before the 17 bytes of
    /// the element, naming how many it held. Nothing is padded.
    pub fn unpack_tuple(key: &[u8]) -> Result<(Self, &[u8]), TupleError> {
        let Some((&code, rest)) = key.split_first() else {
            return refuse(TupleError::Truncated { len: 0 });
        };
        if code != UUID_CODE {
            return refuse(TupleError::WrongCode { found: code });
        }
        let Some((raw, after)) = rest.split_first_chunk::<16>() else {
            return refuse(TupleError::Truncated { len: key.len() });
        };
        Ok((Self::from(*raw), after))
    }
}

/// Hands `error` back, logging it under `trace` first.
fn refuse<T>(error: TupleError) -> Result<T, TupleError> {
    #[cfg(feature = "trace")]
    crate::trace::debug!(%error, "unpack_tuple: returning an error to the caller");
    Err(error)
}

/// Why a key does not lead with a tuple-layer UUID element.
///
/// Variants are stable and machine-readable; callers match on them rather than
/// on the message text, and `#[non_exhaustive]` keeps a future reason an
/// additive change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TupleError {
    /// The element's type code is not the UUID code `0x30`.
    WrongCode {
        /// The type code the key led with.
        found: u8,
    },
    /// The key ends before the 17 bytes of the element.
    Truncated {
        /// How many bytes the key held.
        len: usize,
    },
}

impl fmt::Display for TupleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::WrongCode { found } => write!(
                formatter,
                "tuple element has type code {found:#04x}; a UUID is {UUID_CODE:#04x}"
            ),
            Self::Truncated { len } => write!(
                formatter,
                "tuple key holds {len} bytes; a UUID element is {PACKED_LEN}"
            ),
        }
    }
}

impl Error for TupleError {}
