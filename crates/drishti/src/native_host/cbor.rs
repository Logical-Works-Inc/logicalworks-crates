//! Minimal CBOR reader (RFC 8949 subset) for App Attest objects.
//!
//! Covers exactly the major types the attestation object uses (unsigned int,
//! byte string, text string, array, map). It is NOT a general CBOR codec:
//! unsupported inputs fail closed rather than mis-decode. Inputs over 64 KiB
//! and nesting past 128 levels are refused up front so hostile inputs cannot
//! exhaust memory or the stack.

use std::collections::BTreeMap;

use super::attest::AppAttestError;
use super::ensure;

/// Decoded CBOR value, restricted to the attestation types.
///
/// # Examples
///
/// ```
/// use drishti::native_host::cbor::{CborValue, decode_attestation};
///
/// // 0x01 is the unsigned integer 1; 0x42 0x01 0x02 is a two-byte string.
/// assert_eq!(decode_attestation(&[0x01])?, CborValue::Unsigned(1));
/// assert_eq!(
///     decode_attestation(&[0x42, 0x01, 0x02])?,
///     CborValue::ByteString(vec![1, 2])
/// );
/// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CborValue {
    /// Unsigned integer.
    Unsigned(u64),
    /// Byte string.
    ByteString(Vec<u8>),
    /// UTF-8 text string.
    TextString(String),
    /// Array.
    Array(Vec<CborValue>),
    /// String-keyed map.
    Map(BTreeMap<String, CborValue>),
}

impl CborValue {
    /// Borrow the map, if this is one.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::cbor::decode_attestation;
    ///
    /// // {"a": 1}
    /// let value = decode_attestation(&[0xa1, 0x61, b'a', 0x01])?;
    /// assert_eq!(value.map_value().map(|map| map.len()), Some(1));
    /// assert!(decode_attestation(&[0x01])?.map_value().is_none());
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn map_value(&self) -> Option<&BTreeMap<String, CborValue>> {
        match *self {
            Self::Map(ref map) => Some(map),
            _ => None,
        }
    }

    /// Borrow the array, if this is one.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::cbor::decode_attestation;
    ///
    /// // [1, 2]
    /// let value = decode_attestation(&[0x82, 0x01, 0x02])?;
    /// assert_eq!(value.array_value().map(|items| items.len()), Some(2));
    /// assert!(decode_attestation(&[0x01])?.array_value().is_none());
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn array_value(&self) -> Option<&Vec<CborValue>> {
        match *self {
            Self::Array(ref items) => Some(items),
            _ => None,
        }
    }

    /// Borrow the byte string, if this is one.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::cbor::decode_attestation;
    ///
    /// let value = decode_attestation(&[0x42, 0x07, 0x08])?;
    /// assert_eq!(value.bytes_value(), Some(&vec![7, 8]));
    /// assert!(decode_attestation(&[0x01])?.bytes_value().is_none());
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn bytes_value(&self) -> Option<&Vec<u8>> {
        match *self {
            Self::ByteString(ref bytes) => Some(bytes),
            _ => None,
        }
    }
}

/// Maximum accepted attestation object bytes (real objects are ~1-2 KiB).
///
/// # Examples
///
/// ```
/// use drishti::native_host::attest::AppAttestError;
/// use drishti::native_host::cbor::{MAX_ATTEST_BYTES, decode_attestation};
///
/// // One byte over the cap is refused before any byte is read.
/// let oversized = vec![0x01; MAX_ATTEST_BYTES + 1];
/// assert_eq!(decode_attestation(&oversized), Err(AppAttestError::Truncated));
/// ```
pub const MAX_ATTEST_BYTES: usize = 65_536;

/// Maximum nesting depth accepted while decoding.
const MAX_DEPTH: usize = 128;

/// Decode one attestation object.
///
/// # Errors
///
/// Returns the kernel's [`AppAttestError`] cases: truncated, unsupported
/// major type or additional info, integer overflow, non-map root, and the
/// structural cases the caller raises.
///
/// # Examples
///
/// ```
/// use drishti::native_host::attest::AppAttestError;
/// use drishti::native_host::cbor::{CborValue, decode_attestation};
///
/// assert_eq!(decode_attestation(&[0x18, 0x2a]), Ok(CborValue::Unsigned(42)));
/// // Truncated input and trailing bytes both fail closed.
/// assert_eq!(decode_attestation(&[0x42, 0x01]), Err(AppAttestError::Truncated));
/// assert!(decode_attestation(&[0x01, 0x02]).is_err());
/// ```
pub fn decode_attestation(bytes: &[u8]) -> Result<CborValue, AppAttestError> {
    ensure(bytes.len() <= MAX_ATTEST_BYTES, || {
        AppAttestError::Truncated
    })?;
    let mut reader = CborReader { bytes, position: 0 };
    let value = reader.decode_value(0)?;
    // One object, exactly: bytes after it are a second reading of the same
    // buffer that a verifier of the first would never see.
    ensure(reader.position == bytes.len(), || {
        AppAttestError::UnexpectedType(String::from("trailing bytes"))
    })?;
    Ok(value)
}

/// Cursor over the input bytes.
struct CborReader<'a> {
    /// Input bytes.
    bytes: &'a [u8],
    /// Read position.
    position: usize,
}

impl CborReader<'_> {
    /// Decode one value at the given nesting depth.
    fn decode_value(&mut self, depth: usize) -> Result<CborValue, AppAttestError> {
        ensure(depth <= MAX_DEPTH, || AppAttestError::Truncated)?;
        let initial = self.read_byte()?;
        let major = initial.wrapping_shr(5);
        let info = initial & 0x1f;
        match major {
            0 => Ok(CborValue::Unsigned(self.read_uint(info)?)),
            2 => {
                let length = self.read_length(info)?;
                Ok(CborValue::ByteString(self.read_bytes(length)?))
            }
            3 => {
                let length = self.read_length(info)?;
                let bytes = self.read_bytes(length)?;
                String::from_utf8(bytes)
                    .map(CborValue::TextString)
                    .map_err(AppAttestError::InvalidUtf8)
            }
            4 => {
                let count = self.read_length(info)?;
                self.check_count(count)?;
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(self.decode_value(depth.saturating_add(1))?);
                }
                Ok(CborValue::Array(items))
            }
            5 => {
                let count = self.read_length(info)?;
                self.check_count(count)?;
                let mut map = BTreeMap::new();
                for _ in 0..count {
                    let key = self.decode_value(depth.saturating_add(1))?;
                    let value = self.decode_value(depth.saturating_add(1))?;
                    let text = match key {
                        CborValue::TextString(text) => Ok(text),
                        _ => Err(AppAttestError::UnexpectedType(String::from("map key"))),
                    }?;
                    // A repeated key would let a later value silently replace
                    // the one a signer covered.
                    ensure(map.insert(text, value).is_none(), || {
                        AppAttestError::UnexpectedType(String::from("duplicate map key"))
                    })?;
                }
                Ok(CborValue::Map(map))
            }
            _ => Err(AppAttestError::UnsupportedMajorType(major)),
        }
    }

    /// Decode a head-byte argument: 0-23 immediate, else 1/2/4/8 big-endian
    /// following bytes. Indefinite and reserved forms are unsupported.
    fn read_uint(&mut self, info: u8) -> Result<u64, AppAttestError> {
        match info {
            0..=23 => Ok(u64::from(info)),
            24 => Ok(u64::from(self.read_byte()?)),
            25 => self.read_big_endian(2),
            26 => self.read_big_endian(4),
            27 => self.read_big_endian(8),
            _ => Err(AppAttestError::UnsupportedAdditionalInfo(info)),
        }
    }

    /// Read a byte-string/array/map length, refusing values that cannot be
    /// lengths on this platform.
    fn read_length(&mut self, info: u8) -> Result<usize, AppAttestError> {
        let length = self.read_uint(info)?;
        Ok(usize::try_from(length)?)
    }

    /// Refuse counts that exceed the remaining input: a valid encoding
    /// cannot name more items than bytes remain.
    fn check_count(&self, count: usize) -> Result<(), AppAttestError> {
        let remaining = self.bytes.len().saturating_sub(self.position);
        ensure(count <= remaining, || AppAttestError::Truncated)
    }

    /// Read `count` big-endian bytes as `u64`.
    fn read_big_endian(&mut self, count: usize) -> Result<u64, AppAttestError> {
        let mut value: u64 = 0;
        for _ in 0..count {
            value = value.wrapping_shl(8) | u64::from(self.read_byte()?);
        }
        Ok(value)
    }

    /// Read one byte or fail with truncation.
    fn read_byte(&mut self) -> Result<u8, AppAttestError> {
        let byte = self
            .bytes
            .get(self.position)
            .copied()
            .ok_or(AppAttestError::Truncated)?;
        self.position = self.position.saturating_add(1);
        Ok(byte)
    }

    /// Read `count` bytes or fail with truncation.
    fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>, AppAttestError> {
        let end = self.position.saturating_add(count);
        ensure(end <= self.bytes.len(), || AppAttestError::Truncated)?;
        let bytes = self.bytes[self.position..end].to_vec();
        self.position = end;
        Ok(bytes)
    }
}
