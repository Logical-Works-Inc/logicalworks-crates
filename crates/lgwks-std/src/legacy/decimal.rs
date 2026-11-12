//! `decimal` decodes and encodes COBOL numeric formats exactly.
//!
//! COBOL `COMP-3` (packed) and zoned decimal carry decimal digits in nibbles
//! with an implied scale: `PIC S9(5)V99` stores `12345.67` as the digits
//! `1234567` with scale 2, never as a binary float. This module decodes those
//! bytes into `Decimal`, an `i128` coefficient plus a `u32` scale (IBM
//! Enterprise COBOL Language Reference, numeric items), so `decode` then
//! `encode` returns the same bytes and no `f32`/`f64` ever touches a value.
//! Bad nibbles and signs are typed errors naming the byte.

use std::fmt;

use super::limits::{LimitError, Limits};

/// The sign a packed or zoned field carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sign {
    /// An unsigned field, or a `C`/`F` nibble.
    Positive,
    /// A `D` nibble.
    Negative,
}

/// An exact decimal: a coefficient with an implied scale.
///
/// The value is `coefficient / 10^scale`. The coefficient fits in an `i128`,
/// so a 31-digit COBOL field decodes without loss; the scale is the `V`
/// count of the picture, zero when the picture names none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Decimal {
    /// The unscaled integer value.
    coefficient: i128,
    /// Decimal places the picture implies.
    scale: u32,
}

impl Decimal {
    /// The exact value of a coefficient at a scale.
    #[must_use]
    pub const fn new(coefficient: i128, scale: u32) -> Self {
        Self { coefficient, scale }
    }

    /// The unscaled integer value.
    #[must_use]
    pub(crate) const fn coefficient(self) -> i128 {
        self.coefficient
    }

    /// Decimal places the picture implies.
    #[must_use]
    pub(crate) const fn scale(self) -> u32 {
        self.scale
    }

    /// Whether the coefficient is below zero.
    #[must_use]
    pub(crate) const fn is_negative(self) -> bool {
        self.coefficient < 0
    }
}

/// The exact value in plain notation: `-123.45` for coefficient -12345 at
/// scale 2, `0.05` for 5 at scale 2; never rounded, never in exponent form.
impl fmt::Display for Decimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.coefficient.unsigned_abs().to_string();
        let sign = if self.coefficient < 0 { "-" } else { "" };
        let Ok(places) = usize::try_from(self.scale) else {
            return write!(formatter, "{sign}{digits}e-{}", self.scale);
        };
        if places == 0 {
            return write!(formatter, "{sign}{digits}");
        }
        let padded = format!("{digits:0>width$}", width = places.saturating_add(1));
        let split = padded.len().saturating_sub(places);
        let (whole, fraction) = padded.split_at(split);
        write!(formatter, "{sign}{whole}.{fraction}")
    }
}

/// Why a packed or zoned field could not be decoded or encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecimalError {
    /// A nibble held a value above 9, naming its byte.
    BadNibble {
        /// Zero-based offset of the offending byte.
        offset: usize,
        /// The offending nibble value.
        nibble: u8,
    },
    /// A sign nibble was neither `C`, `D` nor `F`, naming its byte.
    BadSign {
        /// Zero-based offset of the offending byte.
        offset: usize,
        /// The offending nibble value.
        nibble: u8,
    },
    /// A signed value was handed to an unsigned encode, naming the field.
    NegativeUnsigned {
        /// The coefficient that could not be stored unsigned.
        coefficient: i128,
    },
    /// A coefficient needs more digits than the field holds.
    TooWide {
        /// Zero-based offset where the overflow was observed.
        offset: usize,
        /// Digits the coefficient needs.
        needed: usize,
        /// Digits the field holds.
        capacity: usize,
    },
    /// Text that is not a plain decimal number, naming the first bad byte.
    NotANumber {
        /// Zero-based offset of the offending byte in the text.
        offset: usize,
    },
    /// The input or output passed a [`Limits`] ceiling.
    Limit(LimitError),
}

/// Reads plain notation: an optional `-` or `+`, digits, and at most one `.`
/// with digits after it; the scale is the count of digits after the point,
/// so `"123.40"` is coefficient 12340 at scale 2, kept exactly.
impl core::str::FromStr for Decimal {
    type Err = DecimalError;

    fn from_str(text: &str) -> Result<Self, DecimalError> {
        let (negative, body, start) = match (text.strip_prefix('-'), text.strip_prefix('+')) {
            (Some(rest), _) => (true, rest, 1_usize),
            (None, Some(rest)) => (false, rest, 1_usize),
            (None, None) => (false, text, 0_usize),
        };
        let mut coefficient: i128 = 0;
        let mut places: u32 = 0;
        let mut pointed = false;
        let mut digits = 0_usize;
        for (index, byte) in body.bytes().enumerate() {
            let offset = start.saturating_add(index);
            if byte == b'.' && !pointed {
                pointed = true;
                continue;
            }
            if !byte.is_ascii_digit() {
                return crate::legacy::refuse(DecimalError::NotANumber { offset });
            }
            coefficient = fold_digit(coefficient, byte.saturating_sub(b'0'), offset)?;
            digits = digits.saturating_add(1);
            if pointed {
                places = places.saturating_add(1);
            }
        }
        if digits == 0 {
            return crate::legacy::refuse(DecimalError::NotANumber { offset: text.len() });
        }
        let signed = if negative {
            coefficient.saturating_neg()
        } else {
            coefficient
        };
        Ok(Self::new(signed, places))
    }
}

impl fmt::Display for DecimalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NotANumber { offset } => {
                write!(
                    formatter,
                    "byte {offset} is not part of a plain decimal number"
                )
            }
            Self::BadNibble { offset, nibble } => write!(
                formatter,
                "decimal nibble {nibble:#04x} at byte {offset} is not a digit"
            ),
            Self::BadSign { offset, nibble } => write!(
                formatter,
                "decimal sign {nibble:#04x} at byte {offset} is not C, D or F"
            ),
            Self::NegativeUnsigned { coefficient } => write!(
                formatter,
                "coefficient {coefficient} cannot be stored in an unsigned field"
            ),
            Self::TooWide {
                offset,
                needed,
                capacity,
            } => write!(
                formatter,
                "coefficient needs {needed} digits past a {capacity}-digit field at byte {offset}"
            ),
            Self::Limit(ref cause) => {
                write!(formatter, "decimal limits refused the input: {cause}")
            }
        }
    }
}

crate::legacy::limits::limit_carrying_error!(DecimalError);

/// The packed width of a `digits`-digit picture: digits plus sign, paired.
///
/// One sign nibble joins the digits, and nibbles pair into bytes, so the
/// width is the ceiling of half the nibble count. Shared with the copybook
/// layout, which sizes `COMP-3` fields from the same rule.
pub(crate) fn packed_width(digits: usize) -> usize {
    digits.saturating_add(1).div_ceil(2)
}

/// The digit count of a coefficient's magnitude: zero needs one digit.
///
/// Shared with the copybook layout, which checks a value against its picture
/// from the same count.
pub(crate) fn digit_count(value: i128) -> usize {
    let mut magnitude: u128 = value.unsigned_abs();
    if magnitude == 0 {
        return 1;
    }
    let mut count: usize = 0;
    while magnitude > 0 {
        magnitude = magnitude.saturating_div(10);
        count = count.saturating_add(1);
    }
    count
}

/// Reads the magnitude of `value` into `digits` decimal digits, most
/// significant first, zero-padded on the left.
///
/// The caller proves `digits` fits the value; the walk is bounded by it.
/// Shared with the copybook encoder, which renders zoned digits from the
/// same walk.
///
/// # Errors
///
/// [`DecimalError::TooWide`] for a remainder that does not fit a byte, which
/// is unreachable for a remainder below ten and typed rather than defaulted.
pub(crate) fn digits_of(value: i128, digits: usize) -> Result<Vec<u8>, DecimalError> {
    let mut magnitude: u128 = value.unsigned_abs();
    let mut out = vec![0_u8; digits];
    let mut index = digits;
    while index > 0 {
        index = index.saturating_sub(1);
        let low = magnitude.rem_euclid(10);
        // A remainder below ten always fits a byte; the refusal below is
        // unreachable and typed rather than a silent zero.
        let digit: u8 = match u8::try_from(low) {
            Ok(drawn) => drawn,
            Err(_) => {
                let refusal = Err(DecimalError::TooWide {
                    offset: index,
                    needed: digits.saturating_add(1),
                    capacity: digits,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(index, "decimal: refusing a digit past a byte");
                return refusal;
            }
        };
        if let Some(slot) = out.get_mut(index) {
            *slot = digit;
        }
        magnitude = magnitude.saturating_div(10);
    }
    Ok(out)
}

/// Folds one digit nibble into the accumulator, refusing values past 9.
///
/// `offset` is the byte the nibble came from, which is what the error names.
fn fold_digit(accumulator: i128, nibble: u8, offset: usize) -> Result<i128, DecimalError> {
    if nibble > 9 {
        let refusal = Err(DecimalError::BadNibble { offset, nibble });
        #[cfg(feature = "trace")]
        crate::trace::debug!(offset, nibble, "decimal: refusing a non-digit nibble");
        return refusal;
    }
    Ok(accumulator
        .saturating_mul(10)
        .saturating_add(i128::from(nibble)))
}

/// Reads the sign nibble of a packed or zoned field: `C`/`F` positive, `D`
/// negative, anything else a [`DecimalError::BadSign`] at its byte.
fn read_sign(nibble: u8, offset: usize) -> Result<Sign, DecimalError> {
    match nibble {
        0x0c | 0x0f => Ok(Sign::Positive),
        0x0d => Ok(Sign::Negative),
        _ => {
            let refusal = Err(DecimalError::BadSign { offset, nibble });
            #[cfg(feature = "trace")]
            crate::trace::debug!(offset, nibble, "decimal: refusing a bad sign nibble");
            refusal
        }
    }
}

/// Applies a sign to a coefficient for a signed field.
fn apply_sign(value: i128, sign: Sign) -> i128 {
    match sign {
        Sign::Positive => value,
        Sign::Negative => value.saturating_neg(),
    }
}

/// Decodes a COBOL packed (`COMP-3`) field: BCD nibbles with the sign in the
/// low nibble of the last byte, laid out exactly as the compiler lays out a
/// `digits`-digit picture.
///
/// `scale` is the picture's implied places, `signed` whether a `D` sign is
/// admitted. An even picture carries a zero leading pad nibble, which must be
/// zero.
///
/// # Errors
///
/// [`DecimalError::Limit`] past the byte ceiling, [`DecimalError::TooWide`]
/// when the bytes do not match the picture width,
/// [`DecimalError::BadNibble`] for a digit past 9,
/// [`DecimalError::BadSign`] for a sign outside `C`/`D`/`F`.
pub fn decode_packed(
    bytes: &[u8],
    digits: usize,
    scale: u32,
    signed: bool,
    limits: &Limits,
) -> Result<Decimal, DecimalError> {
    check_width(
        bytes,
        packed_width(digits),
        bytes.len().saturating_mul(2),
        digits,
        limits,
    )?;
    let mut coefficient: i128 = 0;
    let last = bytes.len().saturating_sub(1);
    for (offset, byte) in bytes.iter().enumerate() {
        let high = byte.saturating_div(16);
        let low = byte.rem_euclid(16);
        if offset == 0 && digits.rem_euclid(2) == 0 {
            if high != 0 {
                let refusal = Err(DecimalError::BadNibble {
                    offset,
                    nibble: high,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(offset, high, "decimal: refusing a nonzero packed pad");
                return refusal;
            }
            coefficient = fold_digit(coefficient, low, offset)?;
        } else if offset == last {
            coefficient = fold_digit(coefficient, high, offset)?;
            let sign = read_sign(low, offset)?;
            if !signed && sign == Sign::Negative {
                let refusal = Err(DecimalError::NegativeUnsigned { coefficient });
                #[cfg(feature = "trace")]
                crate::trace::debug!(offset, "decimal: refusing a negative unsigned packed value");
                return refusal;
            }
            coefficient = apply_sign(coefficient, sign);
        } else {
            coefficient = fold_digit(coefficient, high, offset)?;
            coefficient = fold_digit(coefficient, low, offset)?;
        }
    }
    Ok(Decimal::new(coefficient, scale))
}

/// Encodes a [`Decimal`] into a COBOL packed (`COMP-3`) field of `digits`
/// picture digits, with the zero pad an even picture needs.
///
/// The sign nibble is the one IBM COBOL writes: `C` or `D` for a signed
/// picture, `F` for an unsigned one, so a record decoded and encoded again
/// keeps the bytes the mainframe wrote.
///
/// # Errors
///
/// [`DecimalError::Limit`] past the byte ceiling,
/// [`DecimalError::NegativeUnsigned`] for a negative unsigned value,
/// [`DecimalError::TooWide`] when the coefficient needs more digits.
pub fn encode_packed(
    value: Decimal,
    digits: usize,
    signed: bool,
    limits: &Limits,
) -> Result<Vec<u8>, DecimalError> {
    check_encodable(value, digits, signed)?;
    let wide = packed_width(digits);
    limits.check_bytes(wide)?;
    let drawn = digits_of(value.coefficient, digits)?;
    let mut nibbles = Vec::with_capacity(digits.saturating_add(1));
    if digits.rem_euclid(2) == 0 {
        nibbles.push(0);
    }
    nibbles.extend(drawn.iter().copied());
    let sign_nibble: u8 = match (signed, value.is_negative()) {
        (false, _) => 0x0f,
        (true, true) => 0x0d,
        (true, false) => 0x0c,
    };
    nibbles.push(sign_nibble);
    let (pairs, remainder) = nibbles.as_chunks::<2>();
    if !remainder.is_empty() {
        let refusal = Err(DecimalError::TooWide {
            offset: 0,
            needed: nibbles.len(),
            capacity: digits,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("decimal: refusing an odd nibble run in a packed encode");
        return refusal;
    }
    let mut out = Vec::with_capacity(wide);
    for &[high, low] in pairs.iter() {
        out.push(high.saturating_mul(16).saturating_add(low));
    }
    Ok(out)
}

/// Refuses `bytes` past the byte ceiling or unlike the picture's `width`;
/// `needed` is the digit count the bytes would carry, which the error names.
fn check_width(
    bytes: &[u8],
    width: usize,
    needed: usize,
    digits: usize,
    limits: &Limits,
) -> Result<(), DecimalError> {
    limits.check_bytes(bytes.len())?;
    if bytes.len() != width {
        let refusal = Err(DecimalError::TooWide {
            offset: 0,
            needed,
            capacity: digits,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(width, "decimal: refusing a field past its picture width");
        return refusal;
    }
    Ok(())
}

/// Refuses a value an encode cannot hold: negative in an unsigned picture,
/// or more digits than the picture carries.
fn check_encodable(value: Decimal, digits: usize, signed: bool) -> Result<(), DecimalError> {
    if !signed && value.is_negative() {
        let refusal = Err(DecimalError::NegativeUnsigned {
            coefficient: value.coefficient,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("decimal: refusing a negative unsigned encode");
        return refusal;
    }
    let needed = digit_count(value.coefficient);
    if needed > digits {
        let refusal = Err(DecimalError::TooWide {
            offset: 0,
            needed,
            capacity: digits,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(needed, digits, "decimal: refusing an over-wide encode");
        return refusal;
    }
    Ok(())
}

/// Decodes a COBOL zoned field of `digits` bytes at scale zero: one digit
/// per byte in the low nibble, the high nibble `F`, except in the last byte
/// of a signed field, where it is the `C`/`D` sign. The copybook reader
/// applies the picture's implied places afterwards.
///
/// # Errors
///
/// [`DecimalError::Limit`] past the byte ceiling, [`DecimalError::TooWide`]
/// when the bytes do not match the picture width,
/// [`DecimalError::BadNibble`] for a digit past 9,
/// [`DecimalError::BadSign`] for a bad zone.
pub(crate) fn decode_zoned(
    bytes: &[u8],
    digits: usize,
    signed: bool,
    limits: &Limits,
) -> Result<Decimal, DecimalError> {
    check_width(bytes, digits, bytes.len(), digits, limits)?;
    let mut coefficient: i128 = 0;
    let last = bytes.len().saturating_sub(1);
    for (offset, byte) in bytes.iter().enumerate() {
        let zone = byte.saturating_div(16);
        let digit = byte.rem_euclid(16);
        coefficient = fold_digit(coefficient, digit, offset)?;
        if offset == last && signed {
            let sign = read_sign(zone, offset)?;
            coefficient = apply_sign(coefficient, sign);
        } else if zone != 0x0f {
            let refusal = Err(DecimalError::BadSign {
                offset,
                nibble: zone,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(offset, zone, "decimal: refusing a zone other than F");
            return refusal;
        }
    }
    Ok(Decimal::new(coefficient, 0))
}

/// Encodes a [`Decimal`] into a COBOL zoned field of `digits` bytes.
///
/// # Errors
///
/// [`DecimalError::Limit`] past the byte ceiling,
/// [`DecimalError::NegativeUnsigned`] for a negative unsigned value,
/// [`DecimalError::TooWide`] when the coefficient needs more digits.
pub(crate) fn encode_zoned(
    value: Decimal,
    digits: usize,
    signed: bool,
    limits: &Limits,
) -> Result<Vec<u8>, DecimalError> {
    limits.check_bytes(digits)?;
    check_encodable(value, digits, signed)?;
    let drawn = digits_of(value.coefficient, digits)?;
    let mut out = Vec::with_capacity(digits);
    let last = digits.saturating_sub(1);
    for (offset, digit) in drawn.iter().enumerate() {
        let zone: u8 = match (offset == last, signed, value.is_negative()) {
            (true, true, true) => 0x0d,
            (true, true, false) => 0x0c,
            _ => 0x0f,
        };
        out.push(zone.saturating_mul(16).saturating_add(*digit));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{Decimal, Limits, decode_packed, decode_zoned, encode_packed, encode_zoned};

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(1024, 1024, 64, 8, 4096);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), super::DecimalError>;

    #[test]
    fn packed_positive_round_trip() -> UnitResult {
        let value = Decimal::new(1_234_567, 2);
        let bytes = encode_packed(value, 7, true, &BOUNDS)?;
        assert_eq!(
            bytes,
            vec![0x12, 0x34, 0x56, 0x7c],
            "12345.67 is seven digits and a sign: 12 34 56 7C"
        );
        assert_eq!(
            decode_packed(&bytes, 7, 2, true, &BOUNDS)?,
            value,
            "the packed bytes must decode back to the value"
        );
        Ok(())
    }

    #[test]
    fn packed_negative_uses_the_d_sign() -> UnitResult {
        let value = Decimal::new(-42, 0);
        let bytes = encode_packed(value, 2, true, &BOUNDS)?;
        assert_eq!(
            bytes,
            vec![0x04, 0x2d],
            "-42 packed in two digits must be 04 2D"
        );
        assert_eq!(
            decode_packed(&bytes, 2, 0, true, &BOUNDS)?,
            value,
            "the signed bytes must decode back negative"
        );
        Ok(())
    }

    #[test]
    fn zoned_unsigned_round_trip() -> UnitResult {
        let value = Decimal::new(789, 0);
        let bytes = encode_zoned(value, 3, false, &BOUNDS)?;
        assert_eq!(
            bytes,
            vec![0xf7, 0xf8, 0xf9],
            "789 zoned unsigned must be F7 F8 F9"
        );
        assert_eq!(
            decode_zoned(&bytes, 3, false, &BOUNDS)?,
            value,
            "the zoned bytes must decode back to the value"
        );
        Ok(())
    }

    #[test]
    fn a_bad_nibble_names_its_byte() {
        assert!(
            matches!(
                decode_packed(&[0x1a, 0x2c], 3, 0, true, &BOUNDS),
                Err(super::DecimalError::BadNibble { offset: 0, .. })
            ),
            "a nibble past 9 must be refused at its byte"
        );
    }

    #[test]
    fn an_over_wide_value_is_refused() {
        assert!(
            encode_packed(Decimal::new(1_000, 0), 3, true, &BOUNDS).is_err(),
            "four digits cannot be stored in a three-digit field"
        );
    }
}
