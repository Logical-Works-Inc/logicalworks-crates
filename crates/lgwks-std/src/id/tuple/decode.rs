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

/// Unpacks one element recursively, tracking depth and total element count.
fn unpack_element_internal<'a>(
    key: &'a [u8],
    depth: usize,
    element_count: &mut usize,
) -> Result<(TupleElement<'a>, &'a [u8]), TupleError> {
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

    let Some((code_ref, rest)) = key.split_first() else {
        return refuse(TupleError::Truncated { len: 0 });
    };
    let code = *code_ref;

    match code {
        NULL_CODE => {
            if depth > 0 && rest.first().copied() == Some(ESCAPE_FF) {
                let after = match rest.get(1..) {
                    Some(slice) => slice,
                    None => &[],
                };
                Ok((TupleElement::Null, after))
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
        TUPLE_CODE => {
            if depth >= MAX_TUPLE_DEPTH {
                return refuse(TupleError::DepthExceeded {
                    limit: MAX_TUPLE_DEPTH,
                });
            }
            let mut children = Vec::new();
            let mut cur_slice = rest;
            let mut terminated = false;
            while let Some(byte_ref) = cur_slice.first() {
                let byte = *byte_ref;
                if byte == 0x00 {
                    if cur_slice.get(1).copied() == Some(ESCAPE_FF) {
                        *element_count = element_count.saturating_add(1);
                        if *element_count > MAX_TUPLE_ELEMENTS {
                            return refuse(TupleError::TooManyElements {
                                limit: MAX_TUPLE_ELEMENTS,
                            });
                        }
                        children.push(TupleElement::Null);
                        cur_slice = match cur_slice.get(2..) {
                            Some(slice) => slice,
                            None => &[],
                        };
                    } else {
                        terminated = true;
                        cur_slice = match cur_slice.get(1..) {
                            Some(slice) => slice,
                            None => &[],
                        };
                        break;
                    }
                } else {
                    let (child, next_cur) =
                        unpack_element_internal(cur_slice, depth.saturating_add(1), element_count)?;
                    children.push(child);
                    cur_slice = next_cur;
                }
            }
            if !terminated {
                return refuse(TupleError::Unterminated { code: TUPLE_CODE });
            }
            Ok((TupleElement::Tuple(children), cur_slice))
        }
        0x0c..=0x13 => {
            let len_u8 = INT_ZERO_CODE.saturating_sub(code);
            let len_usize = usize::from(len_u8);
            let Some(chunk) = rest.get(..len_usize) else {
                return refuse(TupleError::Truncated { len: key.len() });
            };
            if chunk.len() < len_usize {
                return refuse(TupleError::Truncated { len: key.len() });
            }
            let mut encoded = 0_u64;
            for byte_ref in chunk {
                let byte = *byte_ref;
                encoded = (encoded << 8) | u64::from(byte);
            }
            let shift_bits = 8_u32.saturating_mul(u32::from(len_u8));
            let max_n: u64 = if len_u8 == 8_u8 {
                u64::MAX
            } else {
                (1_u64 << shift_bits).saturating_sub(1)
            };
            let mut val_i64 = i64::MIN;
            if let Ok(num) = i64::try_from(i128::from(encoded).saturating_sub(i128::from(max_n))) {
                val_i64 = num;
            }
            let after = match rest.get(len_usize..) {
                Some(slice) => slice,
                None => &[],
            };
            Ok((TupleElement::Int(val_i64), after))
        }
        INT_ZERO_CODE => Ok((TupleElement::Int(0), rest)),
        0x15..=0x1c => {
            let len_u8 = code.saturating_sub(INT_ZERO_CODE);
            let len_usize = usize::from(len_u8);
            let Some(chunk) = rest.get(..len_usize) else {
                return refuse(TupleError::Truncated { len: key.len() });
            };
            if chunk.len() < len_usize {
                return refuse(TupleError::Truncated { len: key.len() });
            }
            let mut unsigned_val = 0_u64;
            for byte_ref in chunk {
                let byte = *byte_ref;
                unsigned_val = (unsigned_val << 8) | u64::from(byte);
            }
            let mut val_i64 = 0_i64;
            if let Ok(num) = i64::try_from(unsigned_val) {
                val_i64 = num;
            }
            let after = match rest.get(len_usize..) {
                Some(slice) => slice,
                None => &[],
            };
            Ok((TupleElement::Int(val_i64), after))
        }
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
