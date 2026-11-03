//! Fixed-width record encoding: values into bytes.
//!
//! [`encode_field`] walks one [`LayoutField`], checking fixed counts,
//! dynamic holder counts and picture widths; numbers encode through the
//! decimal codec, dates from their parsed value, and every failure names
//! its field.
//!
//! [`LayoutField`]: super::model::LayoutField

use super::super::decimal::{Decimal, digit_count, encode_packed, encode_zoned};
use super::super::ebcdic::{self, CodePage};
use super::super::limits::Limits;
use super::SlotKind;
use super::layout::binary_width;
use super::model::{CopybookError, FieldValue, LayoutField};
use super::reader::depending_count;

/// Encodes one field's value at its offset, reading dynamic counts from `numbers`.
pub(crate) fn encode_field(
    field: &LayoutField,
    value: &FieldValue,
    out: &mut [u8],
    numbers: &[(String, i128)],
    limits: &Limits,
    page: CodePage,
) -> Result<(), CopybookError> {
    if let Some(holder) = field.holder() {
        return encode_dynamic(field, value, holder, out, numbers, limits, page);
    }
    if field.max_occurs() > 1 {
        let items = group_items(field, value, "a repeated field needs a group value")?;
        if items.len() != field.max_occurs() {
            let refusal = Err(CopybookError::CountOutOfRange {
                field: String::from(field.name()),
                count: items.len(),
                min: field.max_occurs(),
                max: field.max_occurs(),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a fixed group past its count");
            return refusal;
        }
        for (occurrence, item) in items.iter().enumerate() {
            let at = field
                .offset()
                .saturating_add(occurrence.saturating_mul(field.stride()));
            encode_scalar(field, item, out, at, page)?;
        }
        return Ok(());
    }
    encode_scalar(field, value, out, field.offset(), page)
}

/// Encodes a dynamic field's occurrences, checking the holder count.
fn encode_dynamic(
    field: &LayoutField,
    value: &FieldValue,
    holder: &str,
    out: &mut [u8],
    numbers: &[(String, i128)],
    limits: &Limits,
    page: CodePage,
) -> Result<(), CopybookError> {
    let items = group_items(field, value, "a dynamic field needs a group value")?;
    limits.check_records(items.len())?;
    if items.len() < field.min_occurs() || items.len() > field.max_occurs() {
        let refusal = Err(CopybookError::CountOutOfRange {
            field: String::from(field.name()),
            count: items.len(),
            min: field.min_occurs(),
            max: field.max_occurs(),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a dynamic group past its range");
        return refusal;
    }
    let held = depending_count(holder, numbers, field)?;
    if held != items.len() {
        let refusal = Err(CopybookError::CountOutOfRange {
            field: String::from(field.name()),
            count: items.len(),
            min: held,
            max: held,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a dynamic group past its holder count");
        return refusal;
    }
    if field.children().is_empty() {
        for (occurrence, item) in items.iter().enumerate() {
            let at = field
                .offset()
                .saturating_add(occurrence.saturating_mul(field.stride()));
            encode_scalar(field, item, out, at, page)?;
        }
        return Ok(());
    }
    for (occurrence, item) in items.iter().enumerate() {
        let parts = group_items(
            field,
            item,
            "a dynamic group occurrence needs a group value",
        )?;
        if parts.len() != field.children().len() {
            let refusal = Err(CopybookError::LengthMismatch {
                need: field.children().len(),
                have: parts.len(),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a dynamic occurrence past its template");
            return refusal;
        }
        let base = field
            .offset()
            .saturating_add(occurrence.saturating_mul(field.stride()));
        for (child, part) in field.children().iter().zip(parts.iter()) {
            let at = base.saturating_add(child.offset().saturating_sub(field.offset()));
            encode_scalar(child, part, out, at, page)?;
        }
    }
    Ok(())
}

/// Encodes one scalar value into the record at an absolute offset.
fn encode_scalar(
    field: &LayoutField,
    value: &FieldValue,
    out: &mut [u8],
    at: usize,
    page: CodePage,
) -> Result<(), CopybookError> {
    let Some(window) = out.get_mut(at..at.saturating_add(field.stride())) else {
        let refusal = Err(CopybookError::TruncatedRecord {
            field: String::from(field.name()),
            need: at.saturating_add(field.stride()),
            have: out.len(),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing an encode past the record end");
        return refusal;
    };
    match field.shape() {
        SlotKind::Text => {
            let FieldValue::Text(ref text) = *value else {
                let refusal = Err(mismatch(field, "a text field needs a text value"));
                #[cfg(feature = "trace")]
                crate::trace::debug!("copybook: refusing a text field with a non-text value");
                return refusal;
            };
            let bytes = ebcdic::encode(page, text, &Limits::default())
                .map_err(|cause| mismatch(field, &format!("{cause}")))?;
            if bytes.len() != field.stride() {
                let refusal = Err(CopybookError::ValueMismatch {
                    field: String::from(field.name()),
                    why: format!(
                        "text needs {} characters, holds {}",
                        field.stride(),
                        bytes.len()
                    ),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("copybook: refusing a text value past its width");
                return refusal;
            }
            window.copy_from_slice(&bytes);
            Ok(())
        }
        SlotKind::Zoned { digits, signed } => {
            let number = as_number(field, value)?;
            let bytes = encode_zoned(number, digits, signed, &Limits::default())
                .map_err(|cause| mismatch(field, &format!("{cause}")))?;
            window.copy_from_slice(&bytes);
            Ok(())
        }
        SlotKind::Packed { digits, signed } => {
            let number = as_number(field, value)?;
            let bytes = encode_packed(number, digits, signed, &Limits::default())
                .map_err(|cause| mismatch(field, &format!("{cause}")))?;
            window.copy_from_slice(&bytes);
            Ok(())
        }
        SlotKind::Binary { digits, signed } => {
            let number = as_number(field, value)?;
            let bytes = encode_binary(number, digits, signed, field)?;
            window.copy_from_slice(&bytes);
            Ok(())
        }
    }
}

/// The occurrences of a group value, or a mismatch naming `why`.
fn group_items<'value>(
    field: &LayoutField,
    value: &'value FieldValue,
    why: &str,
) -> Result<&'value [FieldValue], CopybookError> {
    let FieldValue::Group(ref items) = *value else {
        let refusal = Err(mismatch(field, why));
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a group field with a scalar value");
        return refusal;
    };
    Ok(items)
}

/// Reads a numeric or date value as its exact decimal for encoding.
fn as_number(field: &LayoutField, value: &FieldValue) -> Result<Decimal, CopybookError> {
    match *value {
        FieldValue::Number(ref number) => Ok(Decimal::new(number.coefficient(), field.scale())),
        FieldValue::Date(ref dated) => {
            let magnitude = i128::from(dated.year())
                .saturating_mul(10_000)
                .saturating_add(i128::from(dated.month()).saturating_mul(100))
                .saturating_add(i128::from(dated.day()));
            Ok(Decimal::new(magnitude, 0))
        }
        FieldValue::Text(_) | FieldValue::Group(_) => {
            let refusal = Err(mismatch(
                field,
                "a numeric field needs a number or date value",
            ));
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a numeric field with a text value");
            refusal
        }
    }
}

/// A value mismatch naming the field and the reason.
fn mismatch(field: &LayoutField, why: &str) -> CopybookError {
    CopybookError::ValueMismatch {
        field: String::from(field.name()),
        why: String::from(why),
    }
}

/// Encodes an exact decimal into big-endian binary bytes.
fn encode_binary(
    value: Decimal,
    digits: usize,
    signed: bool,
    field: &LayoutField,
) -> Result<Vec<u8>, CopybookError> {
    let needed = digit_count(value.coefficient());
    if needed > digits {
        let refusal = Err(CopybookError::ValueMismatch {
            field: String::from(field.name()),
            why: format!("{needed} digits past a {digits}-digit binary field"),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            needed,
            digits,
            "copybook: refusing an over-wide binary encode"
        );
        return refusal;
    }
    if !signed && value.coefficient() < 0 {
        let refusal = Err(CopybookError::ValueMismatch {
            field: String::from(field.name()),
            why: String::from("an unsigned binary field cannot hold a negative value"),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a negative unsigned binary encode");
        return refusal;
    }
    let width = binary_width(digits);
    if signed {
        let fits = match width {
            2 => i16::try_from(value.coefficient()).is_ok(),
            4 => i32::try_from(value.coefficient()).is_ok(),
            _ => i64::try_from(value.coefficient()).is_ok(),
        };
        if !fits {
            let refusal = Err(CopybookError::ValueMismatch {
                field: String::from(field.name()),
                why: String::from("a signed binary value past its width"),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a signed binary value past its width");
            return refusal;
        }
    }
    let full = value.coefficient().to_be_bytes();
    let start = full.len().saturating_sub(width);
    let Some(window) = full.get(start..) else {
        let refusal = Err(CopybookError::ValueMismatch {
            field: String::from(field.name()),
            why: String::from("a binary value past its width"),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a binary value past its width");
        return refusal;
    };
    Ok(window.to_vec())
}
