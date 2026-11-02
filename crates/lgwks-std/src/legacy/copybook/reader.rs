//! Fixed-width record decoding: bytes into values.
//!
//! [`decode_field`] walks one [`LayoutField`], expanding fixed and dynamic
//! repetition and reading each scalar at its offset; numbers decode through
//! the decimal codec, `DATE` fields keep their raw bytes, and every failure
//! names its field.
//!
//! [`LayoutField`]: super::model::LayoutField

use super::super::decimal::{Decimal, decode_packed, decode_zoned};
use super::super::ebcdic::{self, CodePage};
use super::super::limits::Limits;
use super::SlotKind;
use super::layout::binary_width;
use super::model::{CopybookError, DateValue, FieldValue, LayoutField};

/// Whether a leaf name marks an eight-digit date field.
fn is_date_leaf(field: &LayoutField) -> bool {
    field.name().contains("DATE")
        && matches!(
            field.shape(),
            SlotKind::Zoned {
                digits: 8,
                signed: false
            }
        )
        && field.scale() == 0
}

/// Decodes one field at its offset, reading dynamic counts from `numbers`.
pub(crate) fn decode_field(
    field: &LayoutField,
    bytes: &[u8],
    numbers: &[(String, i128)],
    limits: &Limits,
    page: CodePage,
) -> Result<FieldValue, CopybookError> {
    if let Some(holder) = field.holder() {
        let count = depending_count(holder, numbers, field)?;
        return decode_dynamic(field, bytes, count, limits, page);
    }
    if field.max_occurs() > 1 {
        let mut items = Vec::with_capacity(field.max_occurs());
        for occurrence in 0..field.max_occurs() {
            let at = field
                .offset()
                .saturating_add(occurrence.saturating_mul(field.stride()));
            items.push(decode_scalar(field, bytes, at, page)?);
        }
        return Ok(FieldValue::Group(items));
    }
    decode_scalar(field, bytes, field.offset(), page)
}

/// Reads a dynamic count from the already-decoded numeric fields, refusing
/// a missing holder and a count outside the field's range; the writer checks
/// its values against the same count.
pub(crate) fn depending_count(
    holder: &str,
    numbers: &[(String, i128)],
    field: &LayoutField,
) -> Result<usize, CopybookError> {
    let Some(&(_, count)) = numbers.iter().find(|entry| entry.0 == *holder) else {
        let refusal = Err(CopybookError::DependsOnMissing {
            line: 0,
            field: String::from(field.name()),
            name: String::from(holder),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a dynamic field past a missing count");
        return refusal;
    };
    let Ok(count) = usize::try_from(count) else {
        let shown = if count < 0 {
            0
        } else {
            field.max_occurs().saturating_add(1)
        };
        let refusal = Err(CopybookError::CountOutOfRange {
            field: String::from(field.name()),
            count: shown,
            min: field.min_occurs(),
            max: field.max_occurs(),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a negative or over-wide dynamic count");
        return refusal;
    };
    if count < field.min_occurs() || count > field.max_occurs() {
        let refusal = Err(CopybookError::CountOutOfRange {
            field: String::from(field.name()),
            count,
            min: field.min_occurs(),
            max: field.max_occurs(),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(count, "copybook: refusing a dynamic count past its range");
        return refusal;
    }
    Ok(count)
}

/// Decodes a dynamic field's `count` occurrences.
fn decode_dynamic(
    field: &LayoutField,
    bytes: &[u8],
    count: usize,
    limits: &Limits,
    page: CodePage,
) -> Result<FieldValue, CopybookError> {
    limits.check_records(count)?;
    if field.children().is_empty() {
        let mut items = Vec::with_capacity(count);
        for occurrence in 0..count {
            let at = field
                .offset()
                .saturating_add(occurrence.saturating_mul(field.stride()));
            items.push(decode_scalar(field, bytes, at, page)?);
        }
        return Ok(FieldValue::Group(items));
    }
    let mut occurrences = Vec::with_capacity(count);
    for occurrence in 0..count {
        let base = field
            .offset()
            .saturating_add(occurrence.saturating_mul(field.stride()));
        let mut items = Vec::with_capacity(field.children().len());
        for child in field.children().iter() {
            let at = base.saturating_add(child.offset().saturating_sub(field.offset()));
            items.push(decode_scalar(child, bytes, at, page)?);
        }
        occurrences.push(FieldValue::Group(items));
    }
    Ok(FieldValue::Group(occurrences))
}

/// Decodes one scalar leaf at an absolute offset.
fn decode_scalar(
    field: &LayoutField,
    bytes: &[u8],
    at: usize,
    page: CodePage,
) -> Result<FieldValue, CopybookError> {
    let Some(window) = bytes.get(at..at.saturating_add(field.stride())) else {
        let refusal = Err(CopybookError::TruncatedRecord {
            field: String::from(field.name()),
            need: at.saturating_add(field.stride()),
            have: bytes.len(),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a field past the record end");
        return refusal;
    };
    match field.shape() {
        SlotKind::Text => ebcdic::decode(page, window, &Limits::default())
            .map(FieldValue::Text)
            .map_err(|cause| codec_mismatch(field, &cause)),
        SlotKind::Zoned { digits, signed } => {
            let unsigned = decode_zoned(window, digits, signed, &Limits::default())
                .map_err(|cause| codec_mismatch(field, &cause))?;
            finish_number(field, window, unsigned)
        }
        SlotKind::Packed { digits, signed } => {
            let unsigned = decode_packed(window, digits, 0, signed, &Limits::default())
                .map_err(|cause| codec_mismatch(field, &cause))?;
            finish_number(field, window, unsigned)
        }
        SlotKind::Binary { digits, signed } => {
            let unsigned = decode_binary(window, digits, signed, field)?;
            finish_number(field, window, unsigned)
        }
    }
}

/// A value mismatch carrying the decimal codec's own reason.
fn codec_mismatch(field: &LayoutField, cause: &impl core::fmt::Display) -> CopybookError {
    CopybookError::ValueMismatch {
        field: String::from(field.name()),
        why: format!("{cause}"),
    }
}

/// Wraps a decoded number as a date when the field marks one, else a number.
fn finish_number(
    field: &LayoutField,
    window: &[u8],
    unsigned: Decimal,
) -> Result<FieldValue, CopybookError> {
    let dated = Decimal::new(unsigned.coefficient(), field.scale());
    if is_date_leaf(field) {
        return decode_date(field, window, dated);
    }
    Ok(FieldValue::Number(dated))
}

/// Builds a [`DateValue`] from an eight-digit number, validating month and day.
fn decode_date(
    field: &LayoutField,
    window: &[u8],
    value: Decimal,
) -> Result<FieldValue, CopybookError> {
    let invalid = |text: String| CopybookError::InvalidDate {
        field: String::from(field.name()),
        text,
    };
    let number = value.coefficient();
    if !(0..=99_999_999).contains(&number) {
        let refusal = Err(invalid(format!("{number}")));
        #[cfg(feature = "trace")]
        crate::trace::debug!(number, "copybook: refusing a date past eight digits");
        return refusal;
    }
    let year_drawn = number.saturating_div(10_000);
    let month_drawn = number.rem_euclid(10_000).saturating_div(100);
    let day_drawn = number.rem_euclid(100);
    if !(1..=12).contains(&month_drawn) || !(1..=31).contains(&day_drawn) {
        let refusal = Err(invalid(format!("{number:08}")));
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a date past the calendar");
        return refusal;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        u16::try_from(year_drawn),
        u8::try_from(month_drawn),
        u8::try_from(day_drawn),
    ) else {
        let refusal = Err(invalid(format!("{number:08}")));
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a date past the field width");
        return refusal;
    };
    Ok(FieldValue::Date(DateValue::assemble(
        window.to_vec(),
        year,
        month,
        day,
    )))
}

/// Decodes a big-endian binary integer into an exact decimal.
fn decode_binary(
    window: &[u8],
    digits: usize,
    signed: bool,
    field: &LayoutField,
) -> Result<Decimal, CopybookError> {
    let width = binary_width(digits);
    if window.len() != width {
        let refusal = Err(CopybookError::TruncatedRecord {
            field: String::from(field.name()),
            need: width,
            have: window.len(),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a binary field past its width");
        return refusal;
    }
    let mut magnitude: i128 = 0;
    for byte in window.iter() {
        magnitude = magnitude
            .saturating_mul(256)
            .saturating_add(i128::from(*byte));
    }
    if signed {
        let max_positive: i128 = match width {
            2 => i128::from(i16::MAX),
            4 => i128::from(i32::MAX),
            _ => i128::from(i64::MAX),
        };
        if magnitude > max_positive {
            let modulus = max_positive.saturating_add(1).saturating_mul(2);
            magnitude = magnitude.saturating_sub(modulus);
        }
    }
    Ok(Decimal::new(magnitude, 0))
}
