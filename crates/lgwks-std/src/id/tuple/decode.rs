//! Decoding and unpacking for FoundationDB tuple elements (#851).

use std::borrow::Cow;

use super::{
    BOOL_FALSE_CODE, BOOL_TRUE_CODE, BYTES_CODE, ESCAPE_FF, F32_CODE, F64_CODE, INT_ZERO_CODE,
    MAX_TUPLE_DEPTH, MAX_TUPLE_ELEMENTS, MAX_TUPLE_KEY_BYTES, NULL_CODE, STRING_CODE, TUPLE_CODE,
    TupleElement, TupleError, UUID_CODE, VERSIONSTAMP_80_CODE, VERSIONSTAMP_96_CODE, refuse,
};
use crate::id::Uuid;

/// Reverses sign-bit and one's-complement float encoding.
fn uninvert_float_bytes<const N: usize>(mut bytes: [u8; N]) -> [u8; N] {
    if let Some(first_ref) = bytes.first_mut() {
        if *first_ref & 0x80 == 0 {
            for byte_ref in &mut bytes {
                *byte_ref ^= 0xFF;
            }
        } else {
            *first_ref ^= 0x80;
        }
    }
    bytes
}

/// Unpacks a byte slice with `0x00 0xFF` unescaped to `0x00`.
fn unpack_escaped_slice<'a>(
    rest: &'a [u8],
    code: u8,
) -> Result<(Cow<'a, [u8]>, &'a [u8]), TupleError> {
    let mut idx = 0_usize;
    let mut has_escapes = false;
    let mut terminated = false;
    while let Some(byte_ref) = rest.get(idx) {
        let byte = *byte_ref;
        if byte == 0x00 {
            if rest.get(idx.saturating_add(1)).copied() == Some(ESCAPE_FF) {
                has_escapes = true;
                idx = idx.saturating_add(2);
            } else {
                terminated = true;
                break;
            }
        } else {
            idx = idx.saturating_add(1);
        }
    }
    if !terminated {
        return refuse(TupleError::Unterminated { code });
    }
    let after = match rest.get(idx.saturating_add(1)..) {
        Some(slice) => slice,
        None => &[],
    };
    if !has_escapes {
        let raw_slice = match rest.get(..idx) {
            Some(slice) => slice,
            None => &[],
        };
        Ok((Cow::Borrowed(raw_slice), after))
    } else {
        let mut out_buf = Vec::with_capacity(idx);
        let mut pos = 0_usize;
        while pos < idx {
            if let Some(byte_ref) = rest.get(pos) {
                let byte = *byte_ref;
                if byte == 0x00 && rest.get(pos.saturating_add(1)).copied() == Some(ESCAPE_FF) {
                    out_buf.push(0x00);
                    pos = pos.saturating_add(2);
                } else {
                    out_buf.push(byte);
                    pos = pos.saturating_add(1);
                }
            } else {
                break;
            }
        }
        Ok((Cow::Owned(out_buf), after))
    }
}

/// Unpacks a byte string with `0x00 0xFF` unescaped to `0x00`.
fn unpack_bytes<'a>(rest: &'a [u8]) -> Result<(Cow<'a, [u8]>, &'a [u8]), TupleError> {
    unpack_escaped_slice(rest, BYTES_CODE)
}

/// Unpacks a UTF-8 string with `0x00 0xFF` unescaped to `0x00`.
fn unpack_string<'a>(rest: &'a [u8]) -> Result<(Cow<'a, str>, &'a [u8]), TupleError> {
    let (cow_bytes, after) = unpack_escaped_slice(rest, STRING_CODE)?;
    match cow_bytes {
        Cow::Borrowed(raw_slice) => match std::str::from_utf8(raw_slice) {
            Ok(text_str) => Ok((Cow::Borrowed(text_str), after)),
            Err(err) => refuse(TupleError::InvalidUtf8 {
                at: err.valid_up_to(),
            }),
        },
        Cow::Owned(out_buf) => match String::from_utf8(out_buf) {
            Ok(text_string) => Ok((Cow::Owned(text_string), after)),
            Err(err) => refuse(TupleError::InvalidUtf8 {
                at: err.utf8_error().valid_up_to(),
            }),
        },
    }
}

/// Slices an exact N-byte prefix from `slice` or refuses with a truncation error.
fn split_exact_chunk<const N: usize>(
    slice: &[u8],
    total_len: usize,
) -> Result<(&[u8; N], &[u8]), TupleError> {
    match slice.split_first_chunk::<N>() {
        Some(pair) => Ok(pair),
        None => refuse(TupleError::Truncated { len: total_len }),
    }
}

/// What follows the first `from` bytes of `slice`; empty when it is shorter.
fn tail(slice: &[u8], from: usize) -> &[u8] {
    match slice.get(from..) {
        Some(after) => after,
        None => &[],
    }
}

/// Counts one more element against [`MAX_TUPLE_ELEMENTS`].
fn count_element(element_count: &mut usize) -> Result<(), TupleError> {
    *element_count = element_count.saturating_add(1);
    if *element_count > MAX_TUPLE_ELEMENTS {
        return refuse(TupleError::TooManyElements {
            limit: MAX_TUPLE_ELEMENTS,
        });
    }
    Ok(())
}

/// The `width` big-endian bytes at the front of `rest`: the bytes, their value
/// and what follows them.
fn read_int_bytes(
    rest: &[u8],
    width: u8,
    key_len: usize,
) -> Result<(&[u8], u64, &[u8]), TupleError> {
    let width = usize::from(width);
    let Some(chunk) = rest.get(..width) else {
        return refuse(TupleError::Truncated { len: key_len });
    };
    let mut value = 0_u64;
    for &byte in chunk {
        value = (value << 8) | u64::from(byte);
    }
    Ok((chunk, value, tail(rest, width)))
}

/// Unpacks a negative integer, whose code counts its width down from zero.
fn unpack_negative_int<'a>(
    code: u8,
    rest: &'a [u8],
    key_len: usize,
) -> Result<(TupleElement<'a>, &'a [u8]), TupleError> {
    let width = INT_ZERO_CODE.saturating_sub(code);
    let (chunk, encoded, after) = read_int_bytes(rest, width, key_len)?;
    let shift_bits = 8_u32.saturating_mul(u32::from(width));
    let max_n: u64 = if width == 8_u8 {
        u64::MAX
    } else {
        (1_u64 << shift_bits).saturating_sub(1)
    };
    if chunk.first().copied() == Some(0xff) {
        return refuse(TupleError::NonCanonicalInteger { code });
    }
    let Ok(value) = i64::try_from(i128::from(encoded).saturating_sub(i128::from(max_n))) else {
        return refuse(TupleError::IntegerOutOfRange { code });
    };
    Ok((TupleElement::Int(value), after))
}

/// Unpacks a positive integer, whose code counts its width up from zero.
fn unpack_positive_int<'a>(
    code: u8,
    rest: &'a [u8],
    key_len: usize,
) -> Result<(TupleElement<'a>, &'a [u8]), TupleError> {
    let width = code.saturating_sub(INT_ZERO_CODE);
    let (chunk, unsigned, after) = read_int_bytes(rest, width, key_len)?;
    if chunk.first().copied() == Some(0x00) {
        return refuse(TupleError::NonCanonicalInteger { code });
    }
    let Ok(value) = i64::try_from(unsigned) else {
        return refuse(TupleError::IntegerOutOfRange { code });
    };
    Ok((TupleElement::Int(value), after))
}

/// Unpacks the children of a nested tuple up to its terminating `0x00`.
fn unpack_nested_tuple<'a>(
    rest: &'a [u8],
    depth: usize,
    element_count: &mut usize,
) -> Result<(TupleElement<'a>, &'a [u8]), TupleError> {
    if depth >= MAX_TUPLE_DEPTH {
        return refuse(TupleError::DepthExceeded {
            limit: MAX_TUPLE_DEPTH,
        });
    }
    let mut children = Vec::new();
    let mut cursor = rest;
    while let Some(&first) = cursor.first() {
        if first != 0x00 {
            let (child, next) =
                unpack_element_internal(cursor, depth.saturating_add(1), element_count)?;
            children.push(child);
            cursor = next;
        } else if cursor.get(1).copied() == Some(ESCAPE_FF) {
            count_element(element_count)?;
            children.push(TupleElement::Null);
            cursor = tail(cursor, 2);
        } else {
            return Ok((TupleElement::Tuple(children), tail(cursor, 1)));
        }
    }
    refuse(TupleError::Unterminated { code: TUPLE_CODE })
}

/// Unpacks one element recursively, tracking depth and total element count.
fn unpack_element_internal<'a>(
    key: &'a [u8],
    depth: usize,
    element_count: &mut usize,
) -> Result<(TupleElement<'a>, &'a [u8]), TupleError> {
    count_element(element_count)?;
    if depth > MAX_TUPLE_DEPTH {
        return refuse(TupleError::DepthExceeded {
            limit: MAX_TUPLE_DEPTH,
        });
    }

    let Some((code_ref, rest)) = key.split_first() else {
        return refuse(TupleError::Truncated { len: 0 });
    };
    let code = *code_ref;

    match code {
        NULL_CODE => {
            if depth > 0 && rest.first().copied() == Some(ESCAPE_FF) {
                Ok((TupleElement::Null, tail(rest, 1)))
            } else {
                Ok((TupleElement::Null, rest))
            }
        }
        BYTES_CODE => {
            let (bytes, after) = unpack_bytes(rest)?;
            Ok((TupleElement::Bytes(bytes), after))
        }
        STRING_CODE => {
            let (text, after) = unpack_string(rest)?;
            Ok((TupleElement::String(text), after))
        }
        TUPLE_CODE => unpack_nested_tuple(rest, depth, element_count),
        0x0c..=0x13 => unpack_negative_int(code, rest, key.len()),
        INT_ZERO_CODE => Ok((TupleElement::Int(0), rest)),
        0x15..=0x1c => unpack_positive_int(code, rest, key.len()),
        F32_CODE => {
            let (raw_bytes, after) = split_exact_chunk::<4>(rest, key.len())?;
            let bytes_buf = uninvert_float_bytes(*raw_bytes);
            let float_val = f32::from_bits(u32::from_be_bytes(bytes_buf));
            Ok((TupleElement::F32(float_val), after))
        }
        F64_CODE => {
            let (raw_bytes, after) = split_exact_chunk::<8>(rest, key.len())?;
            let bytes_buf = uninvert_float_bytes(*raw_bytes);
            let float_val = f64::from_bits(u64::from_be_bytes(bytes_buf));
            Ok((TupleElement::F64(float_val), after))
        }
        BOOL_FALSE_CODE => Ok((TupleElement::Bool(false), rest)),
        BOOL_TRUE_CODE => Ok((TupleElement::Bool(true), rest)),
        UUID_CODE => {
            let (raw_bytes, after) = split_exact_chunk::<16>(rest, key.len())?;
            Ok((TupleElement::Uuid(Uuid::from(*raw_bytes)), after))
        }
        VERSIONSTAMP_80_CODE => {
            let (raw_bytes, after) = split_exact_chunk::<10>(rest, key.len())?;
            Ok((TupleElement::Versionstamp80(*raw_bytes), after))
        }
        VERSIONSTAMP_96_CODE => {
            let (raw_bytes, after) = split_exact_chunk::<12>(rest, key.len())?;
            Ok((TupleElement::Versionstamp96(*raw_bytes), after))
        }
        _ => refuse(TupleError::WrongCode { found: code }),
    }
}

/// Unpacks a single FoundationDB tuple element from `key`, returning the element and remaining unconsumed bytes.
///
/// Enforces [`MAX_TUPLE_DEPTH`], [`MAX_TUPLE_ELEMENTS`], and [`MAX_TUPLE_KEY_BYTES`].
#[cfg(test)]
pub(crate) fn unpack_element(key: &[u8]) -> Result<(TupleElement<'_>, &[u8]), TupleError> {
    if key.len() > MAX_TUPLE_KEY_BYTES {
        return refuse(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES,
        });
    }
    let mut element_count = 0;
    unpack_element_internal(key, 0, &mut element_count)
}

/// Unpacks all FoundationDB tuple elements from `key` until end-of-slice.
///
/// Enforces [`MAX_TUPLE_DEPTH`], [`MAX_TUPLE_ELEMENTS`], and [`MAX_TUPLE_KEY_BYTES`].
#[doc(hidden)]
pub fn unpack_elements(key: &[u8]) -> Result<Vec<TupleElement<'_>>, TupleError> {
    if key.len() > MAX_TUPLE_KEY_BYTES {
        return refuse(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES,
        });
    }
    let mut elements = Vec::new();
    let mut cursor = key;
    let mut element_count = 0;
    while !cursor.is_empty() {
        let (element, rest) = unpack_element_internal(cursor, 0, &mut element_count)?;
        elements.push(element);
        cursor = rest;
    }
    Ok(elements)
}
