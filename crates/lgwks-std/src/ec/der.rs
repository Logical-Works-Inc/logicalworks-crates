//! ASN.1 DER encoding and decoding for ECDSA signatures and digest mapping (#840).

use super::EcError;
use super::bignum::U576;
use super::curve::CurveParams;
use super::fail;

/// Encodes a positive integer into minimal ASN.1 DER `INTEGER` TLV bytes.
pub(super) fn encode_der_int(raw_be: &[u8]) -> Vec<u8> {
    let mut start = 0usize;
    while start.saturating_add(1) < raw_be.len() && raw_be.get(start).copied() == Some(0) {
        start = start.saturating_add(1);
    }
    let slice = match raw_be.get(start..) {
        Some(slice_bytes) => slice_bytes,
        None => &[0],
    };
    let needs_pad = match slice.first() {
        Some(val) => *val >= 0x80,
        None => false,
    };
    let content_len = slice.len().saturating_add(usize::from(needs_pad));
    let mut out = Vec::with_capacity(content_len.saturating_add(2));
    out.push(0x02);
    let mut len_u8 = 0_u8;
    if let Ok(num) = u8::try_from(content_len) {
        len_u8 = num;
    }
    out.push(len_u8);
    if needs_pad {
        out.push(0x00);
    }
    out.extend_from_slice(slice);
    out
}

/// Decodes a minimal positive ASN.1 DER `INTEGER` at `offset`.
pub(super) fn decode_der_int(
    der: &[u8],
    offset: usize,
    max_scalar_len: usize,
) -> Result<(U576, usize), EcError> {
    if der.get(offset).copied() != Some(0x02) {
        return fail(EcError::DerEncoding { offset });
    }
    let len_offset = offset.saturating_add(1);
    let Some(int_len_u8) = der.get(len_offset).copied() else {
        return fail(EcError::DerEncoding { offset: len_offset });
    };
    let int_len = usize::from(int_len_u8);
    if int_len == 0 || int_len >= 0x80 {
        return fail(EcError::DerEncoding { offset: len_offset });
    }
    let val_start = offset.saturating_add(2);
    let val_end = val_start.saturating_add(int_len);
    let Some(val_bytes) = der.get(val_start..val_end) else {
        return fail(EcError::DerEncoding { offset: val_start });
    };
    let first = match val_bytes.first() {
        Some(byte_val) => *byte_val,
        None => 0,
    };
    if (first & 0x80) != 0 {
        return fail(EcError::DerEncoding { offset: val_start });
    }
    if first == 0 && val_bytes.len() > 1 {
        let second = match val_bytes.get(1) {
            Some(byte_val) => *byte_val,
            None => 0,
        };
        if (second & 0x80) == 0 {
            return fail(EcError::DerEncoding { offset: val_start });
        }
    }
    let trimmed = if first == 0 && val_bytes.len() > 1 {
        match val_bytes.get(1..) {
            Some(slice) => slice,
            None => val_bytes,
        }
    } else {
        val_bytes
    };
    if trimmed.len() > max_scalar_len {
        return fail(EcError::DerEncoding { offset: val_start });
    }
    Ok((U576::from_be_bytes(trimmed), val_end))
}

/// Converts a message digest into the FIPS 186-5 integer $z$ for `params`.
pub(super) fn digest_to_scalar_z(digest: &[u8], params: &CurveParams) -> U576 {
    let digest_bits = digest.len().saturating_mul(8);
    if digest_bits <= params.bits {
        U576::from_be_bytes(digest).rem_mod(&params.order_n, params.bits)
    } else {
        let mut keep_bytes = 32usize;
        if let Some(bytes_len) = params.bits.saturating_add(7).checked_div(8) {
            keep_bytes = bytes_len;
        }
        let prefix = match digest.get(..keep_bytes) {
            Some(slice) => slice,
            None => digest,
        };
        let mut raw = U576::from_be_bytes(prefix);
        let extra = keep_bytes.saturating_mul(8).saturating_sub(params.bits);
        for _ in 0..extra {
            raw = raw.shr1();
        }
        raw.rem_mod(&params.order_n, params.bits)
    }
}
