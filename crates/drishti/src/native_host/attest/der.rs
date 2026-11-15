//! Minimal DER writer: sequence, integer, UTF-8 string, octet string, with
//! short/long-form lengths.

/// Wrap DER fields in a sequence.
/// # Examples
/// ```
/// use drishti::native_host::attest::der;
/// let pair = der::sequence(&[der::integer(1), der::octet_string(&[9])]);
/// assert_eq!(pair, [0x30, 0x06, 0x02, 0x01, 0x01, 0x04, 0x01, 0x09]);
/// ```
#[must_use]
pub fn sequence(fields: &[Vec<u8>]) -> Vec<u8> {
    let mut content = Vec::new();
    for field in fields {
        content.extend_from_slice(field);
    }
    wrap(0x30, &content)
}

/// Encode a small non-negative integer (used for the quote version,
/// always 1). DER integers are signed, so a value with the high bit set
/// needs a leading zero byte to stay positive.
/// # Examples
/// ```
/// use drishti::native_host::attest::der;
/// assert_eq!(der::integer(1), [0x02, 0x01, 0x01]);
/// assert_eq!(der::integer(0x80), [0x02, 0x02, 0x00, 0x80]);
/// ```
#[must_use]
pub fn integer(value: u8) -> Vec<u8> {
    if value >= 0x80 {
        wrap(0x02, &[0x00, value])
    } else {
        wrap(0x02, &[value])
    }
}

/// Wrap text bytes as a DER `UTF8String` (tag `0x0c`).
/// # Examples
/// ```
/// use drishti::native_host::attest::der;
/// assert_eq!(der::utf8_string("hi"), [0x0c, 0x02, b'h', b'i']);
/// ```
#[must_use]
pub fn utf8_string(value: &str) -> Vec<u8> {
    wrap(0x0c, value.as_bytes())
}

/// Wrap raw bytes as a DER `OCTET STRING` (tag `0x04`).
/// # Examples
/// ```
/// use drishti::native_host::attest::der;
/// assert_eq!(der::octet_string(&[1, 2]), [0x04, 0x02, 1, 2]);
/// // Content past 127 bytes switches to the long length form.
/// assert_eq!(der::octet_string(&[0; 200])[..3], [0x04, 0x81, 200]);
/// ```
#[must_use]
pub fn octet_string(value: &[u8]) -> Vec<u8> {
    wrap(0x04, value)
}

/// Tag plus length plus content.
fn wrap(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len().saturating_add(5));
    out.push(tag);
    out.extend_from_slice(&length_bytes(content.len()));
    out.extend_from_slice(content);
    out
}

/// Short/long-form length bytes. Lengths that do not fit `u8` are
/// refused by the caller-facing encoders; this helper only runs on
/// values the callers proved in range.
fn length_bytes(length: usize) -> Vec<u8> {
    if length < 128 {
        // Lengths below 128 always fit in `u8`: the low byte is the
        // whole value, with no conversion to fail.
        return vec![length.to_le_bytes()[0]];
    }
    let mut value = length;
    let mut bytes = Vec::new();
    while value > 0 {
        // The mask keeps the low 8 bits, which always fit in `u8`.
        bytes.insert(0, (value & 0xff).to_le_bytes()[0]);
        value >>= 8;
    }
    // A `usize` needs at most 8 length bytes: the low byte is the
    // whole count, with no conversion to fail.
    let count = bytes.len().to_le_bytes()[0];
    let mut out = vec![0x80 | count];
    out.extend_from_slice(&bytes);
    out
}
