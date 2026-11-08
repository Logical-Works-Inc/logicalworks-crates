//! The FoundationDB tuple-layer element for [`Uuid`] (#851, INV-ID-5): type
//! code `0x30` followed by the 16 bytes in network order, the encoding the
//! tuple layer's specification fixes (`design/tuple.md` in apple/foundationdb,
//! "UUID: typecode 0x30, length 16 bytes, network byte order") and the one
//! `foundationdb-tuple` writes for `uuid::Uuid`. Network order makes the packed
//! bytes sort as the identifiers do, so a key that leads with a version 7
//! identifier sorts in issue order in the store.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use super::Uuid;

pub(crate) mod decode;
#[cfg(test)]
pub(crate) use decode::unpack_element;
#[doc(hidden)]
pub use decode::unpack_elements;

/// The maximum allowed nesting depth for tuples (32).
#[doc(hidden)]
pub(crate) const MAX_TUPLE_DEPTH: usize = 32;

/// The maximum allowed number of elements in a tuple (1,024).
#[doc(hidden)]
pub(crate) const MAX_TUPLE_ELEMENTS: usize = 1_024;

/// The maximum allowed byte length for a packed tuple key (10,000).
#[doc(hidden)]
pub(crate) const MAX_TUPLE_KEY_BYTES: usize = 10_000;

/// The tuple layer's type code for a null element (`0x00`).
pub(super) const NULL_CODE: u8 = 0x00;

/// The tuple layer's type code for a byte-string element (`0x01`).
pub(super) const BYTES_CODE: u8 = 0x01;

/// The tuple layer's type code for a UTF-8 string element (`0x02`).
pub(super) const STRING_CODE: u8 = 0x02;

/// The tuple layer's type code for a nested tuple element (`0x05`).
pub(super) const TUPLE_CODE: u8 = 0x05;

/// The tuple layer's base type code for the integer zero (`0x14`).
pub(super) const INT_ZERO_CODE: u8 = 0x14;

/// The tuple layer's type code for a 32-bit float element (`0x20`).
pub(super) const F32_CODE: u8 = 0x20;

/// The tuple layer's type code for a 64-bit float element (`0x21`).
pub(super) const F64_CODE: u8 = 0x21;

/// The tuple layer's type code for a boolean false element (`0x26`).
pub(super) const BOOL_FALSE_CODE: u8 = 0x26;

/// The tuple layer's type code for a boolean true element (`0x27`).
pub(super) const BOOL_TRUE_CODE: u8 = 0x27;

/// The tuple layer's type code for a UUID element.
pub(super) const UUID_CODE: u8 = 0x30;

/// The tuple layer's type code for an 80-bit versionstamp element (`0x32`).
pub(super) const VERSIONSTAMP_80_CODE: u8 = 0x32;

/// The tuple layer's type code for a 96-bit versionstamp element (`0x33`).
pub(super) const VERSIONSTAMP_96_CODE: u8 = 0x33;

/// The escape byte following `0x00` inside byte strings, UTF-8 strings, and nested tuples (`0xFF`).
pub(super) const ESCAPE_FF: u8 = 0xFF;

/// The width of a packed element: the code and the 16 bytes.
pub(super) const PACKED_LEN: usize = 17;

/// A FoundationDB tuple-layer element (#851).
///
/// Encodes data types into a binary representation preserving natural sort order
/// across keys in the store according to the FoundationDB tuple specification.
#[derive(Debug, Clone, PartialEq, PartialOrd)]
#[non_exhaustive]
#[doc(hidden)]
pub enum TupleElement<'a> {
    /// A null value (`0x00` at top level; `0x00 0xFF` inside a nested tuple).
    Null,
    /// Arbitrary binary bytes (`0x01` + escaped bytes + `0x00`).
    Bytes(Cow<'a, [u8]>),
    /// A UTF-8 encoded string (`0x02` + escaped bytes + `0x00`).
    String(Cow<'a, str>),
    /// A nested tuple (`0x05` + nested elements + `0x00`), bounded by [`MAX_TUPLE_DEPTH`].
    Tuple(Vec<TupleElement<'a>>),
    /// A 64-bit signed integer (`0x0c`..=`0x1c` preserving signed order).
    Int(i64),
    /// A 32-bit IEEE 754 floating-point number (`0x20` preserving float order).
    F32(f32),
    /// A 64-bit IEEE 754 floating-point number (`0x21` preserving float order).
    F64(f64),
    /// A boolean value (`0x26` for false, `0x27` for true).
    Bool(bool),
    /// A 128-bit UUID (`0x30` + 16 bytes in network byte order).
    Uuid(Uuid),
    /// An 80-bit versionstamp (`0x32` + 10 bytes).
    Versionstamp80([u8; 10]),
    /// A 96-bit versionstamp (`0x33` + 12 bytes).
    Versionstamp96([u8; 12]),
}

/// Backwards-compatible alias for [`TupleElement`].
#[doc(hidden)]
pub type Element<'a> = TupleElement<'a>;

impl Uuid {
    /// Appends this identifier to `key` as one tuple-layer element: `0x30`
    /// and the 16 bytes in network order.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_std::id::Uuid;
    ///
    /// let id = Uuid::from([0u8; 16]);
    /// let mut key = Vec::new();
    /// id.pack_tuple(&mut key);
    /// assert_eq!(key.len(), 17);
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_std::id::Uuid;
    ///
    /// let id = Uuid::from([0u8; 16]);
    /// let mut key = Vec::new();
    /// id.pack_tuple(&mut key);
    /// if let Ok((unpacked, tail)) = Uuid::unpack_tuple(&key) {
    ///     assert_eq!(unpacked, id);
    ///     assert!(tail.is_empty());
    /// }
    /// ```
    pub fn unpack_tuple(key: &[u8]) -> Result<(Self, &[u8]), TupleError> {
        let Some((code_ref, rest)) = key.split_first() else {
            return refuse(TupleError::Truncated { len: 0 });
        };
        let code = *code_ref;
        if code != UUID_CODE {
            return refuse(TupleError::WrongCode { found: code });
        }
        let Some((raw, after)) = rest.split_first_chunk::<16>() else {
            return refuse(TupleError::Truncated { len: key.len() });
        };
        Ok((Self::from(*raw), after))
    }
}

/// Encodes a single integer into big-endian bytes according to the FoundationDB tuple specification.
fn pack_int(val: i64, key_buf: &mut Vec<u8>) {
    if val == 0 {
        key_buf.push(INT_ZERO_CODE);
    } else if val > 0 {
        let mut unsigned = 0_u64;
        if let Ok(num) = u64::try_from(val) {
            unsigned = num;
        }
        let len_u8: u8 = if unsigned <= 0xFF {
            1_u8
        } else if unsigned <= 0xFFFF {
            2_u8
        } else if unsigned <= 0xFF_FFFF {
            3_u8
        } else if unsigned <= 0xFFFF_FFFF {
            4_u8
        } else if unsigned <= 0xFF_FFFF_FFFF {
            5_u8
        } else if unsigned <= 0xFFFF_FFFF_FFFF {
            6_u8
        } else if unsigned <= 0xFF_FFFF_FFFF_FFFF {
            7_u8
        } else {
            8_u8
        };
        key_buf.push(INT_ZERO_CODE.saturating_add(len_u8));
        let be_bytes = unsigned.to_be_bytes();
        let len_usize = usize::from(len_u8);
        if let Some(chunk) = be_bytes.get(8_usize.saturating_sub(len_usize)..) {
            key_buf.extend_from_slice(chunk);
        }
    } else {
        let len_u8: u8 = if val >= -255 {
            1_u8
        } else if val >= -65535 {
            2_u8
        } else if val >= -16777215 {
            3_u8
        } else if val >= -4294967295 {
            4_u8
        } else if val >= -1099511627775 {
            5_u8
        } else if val >= -281474976710655 {
            6_u8
        } else if val >= -72057594037927935 {
            7_u8
        } else {
            8_u8
        };
        key_buf.push(INT_ZERO_CODE.saturating_sub(len_u8));
        let shift_bits = 8_u32.saturating_mul(u32::from(len_u8));
        let max_n: u64 = if len_u8 == 8_u8 {
            u64::MAX
        } else {
            (1_u64 << shift_bits).saturating_sub(1)
        };
        let mut encoded = 0_u64;
        if let Ok(num) = u64::try_from(i128::from(max_n).saturating_add(i128::from(val))) {
            encoded = num;
        }
        let be_bytes = encoded.to_be_bytes();
        let len_usize = usize::from(len_u8);
        if let Some(chunk) = be_bytes.get(8_usize.saturating_sub(len_usize)..) {
            key_buf.extend_from_slice(chunk);
        }
    }
}

fn invert_float_bytes(bytes: &mut [u8]) {
    if let Some(first_ref) = bytes.first_mut() {
        if *first_ref & 0x80 != 0 {
            for byte_ref in bytes.iter_mut() {
                *byte_ref ^= 0xFF;
            }
        } else {
            *first_ref ^= 0x80;
        }
    }
}

/// Encodes a 32-bit float into IEEE 754 big-endian bytes with order-preserving bit inversion.
fn pack_f32(fl: f32, key_buf: &mut Vec<u8>) {
    key_buf.push(F32_CODE);
    let mut bytes = fl.to_bits().to_be_bytes();
    invert_float_bytes(&mut bytes);
    key_buf.extend_from_slice(&bytes);
}

/// Encodes a 64-bit float into IEEE 754 big-endian bytes with order-preserving bit inversion.
fn pack_f64(fl: f64, key_buf: &mut Vec<u8>) {
    key_buf.push(F64_CODE);
    let mut bytes = fl.to_bits().to_be_bytes();
    invert_float_bytes(&mut bytes);
    key_buf.extend_from_slice(&bytes);
}

fn escape_and_push_bytes(code: u8, slice: &[u8], key_buf: &mut Vec<u8>) {
    key_buf.push(code);
    for byte_ref in slice {
        let byte = *byte_ref;
        if byte == 0x00 {
            key_buf.push(0x00);
            key_buf.push(ESCAPE_FF);
        } else {
            key_buf.push(byte);
        }
    }
    key_buf.push(0x00);
}

/// Encodes a byte sequence with `0x00` escaped as `0x00 0xFF` and terminated by `0x00`.
fn pack_bytes(bytes: &[u8], key_buf: &mut Vec<u8>) {
    escape_and_push_bytes(BYTES_CODE, bytes, key_buf);
}

/// Encodes a UTF-8 string with `0x00` escaped as `0x00 0xFF` and terminated by `0x00`.
fn pack_string(text: &str, key_buf: &mut Vec<u8>) {
    escape_and_push_bytes(STRING_CODE, text.as_bytes(), key_buf);
}

/// Packs an element recursively, tracking depth and total element count.
fn pack_element_internal(
    element: &TupleElement<'_>,
    key_buf: &mut Vec<u8>,
    depth: usize,
    element_count: &mut usize,
) -> Result<(), TupleError> {
    *element_count = element_count.saturating_add(1);
    if *element_count > MAX_TUPLE_ELEMENTS {
        return refuse(TupleError::TooManyElements {
            limit: MAX_TUPLE_ELEMENTS,
        });
    }
    if depth > MAX_TUPLE_DEPTH {
        return refuse(TupleError::DepthExceeded {
            limit: MAX_TUPLE_DEPTH,
        });
    }
    if key_buf.len() > MAX_TUPLE_KEY_BYTES {
        return refuse(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES,
        });
    }

    match *element {
        TupleElement::Null => {
            if depth > 0 {
                key_buf.push(NULL_CODE);
                key_buf.push(ESCAPE_FF);
            } else {
                key_buf.push(NULL_CODE);
            }
        }
        TupleElement::Bytes(ref bytes) => {
            pack_bytes(bytes.as_ref(), key_buf);
        }
        TupleElement::String(ref text) => {
            pack_string(text.as_ref(), key_buf);
        }
        TupleElement::Tuple(ref nested) => {
            if depth >= MAX_TUPLE_DEPTH {
                return refuse(TupleError::DepthExceeded {
                    limit: MAX_TUPLE_DEPTH,
                });
            }
            key_buf.push(TUPLE_CODE);
            for child in nested {
                pack_element_internal(child, key_buf, depth.saturating_add(1), element_count)?;
            }
            key_buf.push(0x00);
        }
        TupleElement::Int(val) => {
            pack_int(val, key_buf);
        }
        TupleElement::F32(fl) => {
            pack_f32(fl, key_buf);
        }
        TupleElement::F64(fl) => {
            pack_f64(fl, key_buf);
        }
        TupleElement::Bool(flag) => {
            if flag {
                key_buf.push(BOOL_TRUE_CODE);
            } else {
                key_buf.push(BOOL_FALSE_CODE);
            }
        }
        TupleElement::Uuid(id) => {
            key_buf.push(UUID_CODE);
            key_buf.extend_from_slice(id.as_bytes());
        }
        TupleElement::Versionstamp80(ref vs) => {
            key_buf.reserve(11);
            key_buf.push(VERSIONSTAMP_80_CODE);
            key_buf.extend_from_slice(vs);
        }
        TupleElement::Versionstamp96(ref vs) => {
            key_buf.reserve(13);
            key_buf.push(VERSIONSTAMP_96_CODE);
            key_buf.extend_from_slice(vs);
        }
    }

    if key_buf.len() > MAX_TUPLE_KEY_BYTES {
        return refuse(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES,
        });
    }

    Ok(())
}

/// Appends a single FoundationDB tuple element to `key_buf`.
///
/// Enforces [`MAX_TUPLE_DEPTH`], [`MAX_TUPLE_ELEMENTS`], and [`MAX_TUPLE_KEY_BYTES`].
pub(crate) fn pack_element(
    element: &TupleElement<'_>,
    key_buf: &mut Vec<u8>,
) -> Result<(), TupleError> {
    let mut element_count = 0;
    pack_element_internal(element, key_buf, 0, &mut element_count)?;
    if key_buf.len() > MAX_TUPLE_KEY_BYTES {
        return refuse(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES,
        });
    }
    Ok(())
}

/// Appends a slice of FoundationDB tuple elements to `key_buf`.
///
/// Enforces [`MAX_TUPLE_DEPTH`], [`MAX_TUPLE_ELEMENTS`], and [`MAX_TUPLE_KEY_BYTES`].
#[doc(hidden)]
pub fn pack_elements(
    elements: &[TupleElement<'_>],
    key_buf: &mut Vec<u8>,
) -> Result<(), TupleError> {
    if elements.len() > MAX_TUPLE_ELEMENTS {
        return refuse(TupleError::TooManyElements {
            limit: MAX_TUPLE_ELEMENTS,
        });
    }
    if elements.len() == 1
        && let Some(first) = elements.first()
    {
        return pack_element(first, key_buf);
    }
    let mut element_count = 0;
    for element in elements {
        pack_element_internal(element, key_buf, 0, &mut element_count)?;
    }
    if key_buf.len() > MAX_TUPLE_KEY_BYTES {
        return refuse(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES,
        });
    }
    Ok(())
}

/// Hands `error` back, logging it under `trace` first.
pub(super) fn refuse<T>(error: TupleError) -> Result<T, TupleError> {
    #[cfg(feature = "trace")]
    crate::trace::debug!(%error, "tuple: returning an error to the caller");
    Err(error)
}

/// Why a key does not lead with a valid tuple-layer element.
///
/// Variants are stable and machine-readable; callers match on them rather than
/// on the message text, and `#[non_exhaustive]` keeps a future reason an
/// additive change.
///
/// # Examples
///
/// ```
/// use lgwks_std::id::TupleError;
///
/// let err = TupleError::InvalidUtf8 { at: 0 };
/// assert!(matches!(err, TupleError::InvalidUtf8 { .. }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TupleError {
    /// The element's type code is not recognized or not expected.
    WrongCode {
        /// The type code the key led with.
        found: u8,
    },
    /// The key ends before the required bytes of the element.
    Truncated {
        /// How many bytes the key held.
        len: usize,
    },
    /// A nested tuple exceeded the maximum nesting depth.
    DepthExceeded {
        /// The nesting depth limit that was exceeded.
        limit: usize,
    },
    /// A tuple exceeded the maximum element count.
    TooManyElements {
        /// The element count limit that was exceeded.
        limit: usize,
    },
    /// A packed tuple key exceeded the maximum allowed byte length.
    KeyTooLarge {
        /// The key byte length limit that was exceeded.
        limit: usize,
    },
    /// A string element contains bytes that are not valid UTF-8.
    InvalidUtf8 {
        /// Byte offset of the first invalid UTF-8 sequence.
        at: usize,
    },
    /// An element encoding was not terminated before the key ended.
    Unterminated {
        /// The type code of the unterminated element.
        code: u8,
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
            Self::DepthExceeded { limit } => {
                write!(formatter, "tuple nesting depth exceeded limit of {limit}")
            }
            Self::TooManyElements { limit } => {
                write!(formatter, "tuple element count exceeded limit of {limit}")
            }
            Self::KeyTooLarge { limit } => {
                write!(formatter, "tuple key size exceeded limit of {limit} bytes")
            }
            Self::InvalidUtf8 { at } => write!(
                formatter,
                "tuple string element contains invalid UTF-8 at byte offset {at}"
            ),
            Self::Unterminated { code } => write!(
                formatter,
                "tuple element with type code {code:#04x} is unterminated"
            ),
        }
    }
}

impl Error for TupleError {}
