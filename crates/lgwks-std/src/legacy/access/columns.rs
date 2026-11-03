//! Table columns: cell decoding and encoding.
//!
//! [`read_cell`] turns the bytes at a cursor into one [`CellValue`] of the
//! column's storage; [`encode_cell`] renders a value back. Text trims its
//! padding, currency is an exact decimal of scale 4, and memo cells with no
//! bytes are nulls.
//!
//! [`CellValue`]: super::CellValue

use super::super::decimal::Decimal;
use super::catalog::Column;
use super::{AccessError, CellValue, ColumnDef, MILLIS_PER_DAY};
use crate::legacy::get_u32_le;

/// Reads one cell of the column's storage.
pub(crate) fn read_cell(
    window: &[u8],
    at: usize,
    table: &str,
    column: &Column,
) -> Result<(CellValue, usize), AccessError> {
    let bad = |why: String| AccessError::ValueMismatch {
        table: String::from(table),
        column: column.name.clone(),
        why,
    };
    match column.tag {
        super::ColumnTag::Text => {
            let Some(bytes) = window.get(at..at.saturating_add(column.len)) else {
                return crate::legacy::refuse(bad(format!(
                    "text needs {} bytes past the page",
                    column.len
                )));
            };
            let trimmed = trim_padding(bytes, table, &column.name)?;
            match String::from_utf8(trimmed.to_vec()) {
                Ok(text) => Ok((CellValue::Text(text), at.saturating_add(column.len))),
                Err(_) => Err(bad(String::from("text is not UTF-8"))),
            }
        }
        super::ColumnTag::Integer => {
            let Some(bytes) = window.get(at..at.saturating_add(4)) else {
                return crate::legacy::refuse(bad(String::from(
                    "integer needs 4 bytes past the page",
                )));
            };
            let Ok(fixed) = <[u8; 4]>::try_from(bytes) else {
                return crate::legacy::refuse(bad(String::from("integer past its window")));
            };
            Ok((
                CellValue::Integer(i32::from_be_bytes(fixed)),
                at.saturating_add(4),
            ))
        }
        super::ColumnTag::Currency => {
            let Some(bytes) = window.get(at..at.saturating_add(8)) else {
                return crate::legacy::refuse(bad(String::from(
                    "currency needs 8 bytes past the page",
                )));
            };
            let Ok(fixed) = <[u8; 8]>::try_from(bytes) else {
                return crate::legacy::refuse(bad(String::from("currency past its window")));
            };
            Ok((
                CellValue::Currency(Decimal::new(i128::from(i64::from_be_bytes(fixed)), 4)),
                at.saturating_add(8),
            ))
        }
        super::ColumnTag::Date => {
            let Some(bytes) = window.get(at..at.saturating_add(8)) else {
                return crate::legacy::refuse(bad(String::from(
                    "date needs 8 bytes past the page",
                )));
            };
            let Ok(fixed) = <[u8; 8]>::try_from(bytes) else {
                return crate::legacy::refuse(bad(String::from("date past its window")));
            };
            let (Some(day_window), Some(millis_window)) = (fixed.get(..4), fixed.get(4..)) else {
                return crate::legacy::refuse(bad(String::from("date past its window")));
            };
            let mut day_bytes = [0_u8; 4];
            let mut millis_bytes = [0_u8; 4];
            day_bytes.copy_from_slice(day_window);
            millis_bytes.copy_from_slice(millis_window);
            let days = i32::from_be_bytes(day_bytes);
            let millis = u32::from_be_bytes(millis_bytes);
            if millis >= MILLIS_PER_DAY {
                return crate::legacy::refuse(bad(String::from("date milliseconds past one day")));
            }
            Ok((CellValue::Date { days, millis }, at.saturating_add(8)))
        }
        super::ColumnTag::Memo => {
            let Some(len) = get_u32_le(window, at) else {
                return crate::legacy::refuse(bad(String::from(
                    "memo needs a length past the page",
                )));
            };
            if len == 0 {
                return Ok((CellValue::Null, at.saturating_add(4)));
            }
            let Ok(len) = usize::try_from(len) else {
                return crate::legacy::refuse(bad(String::from(
                    "memo length past the address width",
                )));
            };
            let body_at = at.saturating_add(4);
            let Some(bytes) = window.get(body_at..body_at.saturating_add(len)) else {
                return crate::legacy::refuse(bad(String::from("memo runs past the page")));
            };
            match String::from_utf8(bytes.to_vec()) {
                Ok(text) => Ok((CellValue::Memo(text), body_at.saturating_add(len))),
                Err(_) => Err(bad(String::from("memo is not UTF-8"))),
            }
        }
    }
}

/// Trims trailing spaces and NULs from a fixed text field.
fn trim_padding<'a>(bytes: &'a [u8], table: &str, column: &str) -> Result<&'a [u8], AccessError> {
    let mut end = bytes.len();
    while end > 0 {
        let Some(&last) = bytes.get(end.saturating_sub(1)) else {
            break;
        };
        if last == 0x20 || last == 0x00 {
            end = end.saturating_sub(1);
        } else {
            break;
        }
    }
    let Some(trimmed) = bytes.get(..end) else {
        return crate::legacy::refuse(AccessError::ValueMismatch {
            table: String::from(table),
            column: String::from(column),
            why: String::from("text runs past the page"),
        });
    };
    Ok(trimmed)
}

/// Copies bytes into a page at an offset the caller has already bounded.
fn place_bytes(
    page: &mut [u8],
    at: usize,
    bytes: &[u8],
    table: &str,
    column: &str,
) -> Result<(), AccessError> {
    let Some(window) = page.get_mut(at..at.saturating_add(bytes.len())) else {
        return crate::legacy::refuse(AccessError::ValueMismatch {
            table: String::from(table),
            column: String::from(column),
            why: String::from("cell runs past the page"),
        });
    };
    window.copy_from_slice(bytes);
    Ok(())
}

/// Encodes one cell at the cursor, returning the cursor past it.
pub(crate) fn encode_cell(
    page: &mut [u8],
    cursor: usize,
    table: &str,
    column: &ColumnDef,
    cell: &CellValue,
    page_size: usize,
) -> Result<usize, AccessError> {
    let bad = |why: String| AccessError::ValueMismatch {
        table: String::from(table),
        column: column.name.clone(),
        why,
    };
    let column_name = column.name.clone();
    match column.tag {
        super::ColumnTag::Text => {
            let CellValue::Text(ref text) = *cell else {
                return crate::legacy::refuse(bad(String::from("a text column needs text")));
            };
            let width = usize::from(column.len);
            if text.len() > width {
                return crate::legacy::refuse(bad(format!(
                    "text needs {} bytes past {width}",
                    text.len()
                )));
            }
            let end = cursor.saturating_add(width);
            if end > page_size {
                return crate::legacy::refuse(bad(String::from("text runs past the page")));
            }
            if let Some(window) = page.get_mut(cursor..end) {
                for slot in window.iter_mut() {
                    *slot = 0x20;
                }
            } else {
                return crate::legacy::refuse(bad(String::from("text runs past the page")));
            }
            place_bytes(page, cursor, text.as_bytes(), table, &column_name)?;
            Ok(end)
        }
        super::ColumnTag::Integer => {
            let CellValue::Integer(ref value) = *cell else {
                return crate::legacy::refuse(bad(String::from(
                    "an integer column needs an integer",
                )));
            };
            let end = cursor.saturating_add(4);
            if end > page_size {
                return crate::legacy::refuse(bad(String::from("integer runs past the page")));
            }
            let bytes = value.to_be_bytes();
            place_bytes(page, cursor, &bytes, table, &column_name)?;
            Ok(end)
        }
        super::ColumnTag::Currency => {
            let CellValue::Currency(ref value) = *cell else {
                return crate::legacy::refuse(bad(String::from(
                    "a currency column needs currency",
                )));
            };
            if value.scale() != 4 {
                return crate::legacy::refuse(bad(String::from("currency needs scale 4")));
            }
            let Ok(magnitude) = i64::try_from(value.coefficient()) else {
                return crate::legacy::refuse(bad(String::from("currency past 64 bits")));
            };
            let end = cursor.saturating_add(8);
            if end > page_size {
                return crate::legacy::refuse(bad(String::from("currency runs past the page")));
            }
            let bytes = magnitude.to_be_bytes();
            place_bytes(page, cursor, &bytes, table, &column_name)?;
            Ok(end)
        }
        super::ColumnTag::Date => {
            let CellValue::Date {
                ref days,
                ref millis,
            } = *cell
            else {
                return crate::legacy::refuse(bad(String::from("a date column needs a date")));
            };
            if *millis >= MILLIS_PER_DAY {
                return crate::legacy::refuse(bad(String::from("date milliseconds past one day")));
            }
            let end = cursor.saturating_add(8);
            if end > page_size {
                return crate::legacy::refuse(bad(String::from("date runs past the page")));
            }
            let day_bytes = days.to_be_bytes();
            let millis_bytes = millis.to_be_bytes();
            place_bytes(page, cursor, &day_bytes, table, &column_name)?;
            place_bytes(
                page,
                cursor.saturating_add(4),
                &millis_bytes,
                table,
                &column_name,
            )?;
            Ok(end)
        }
        super::ColumnTag::Memo => match *cell {
            CellValue::Memo(ref text) => {
                let end = cursor.saturating_add(4).saturating_add(text.len());
                if end > page_size {
                    return crate::legacy::refuse(bad(String::from("memo runs past the page")));
                }
                let Ok(len) = u32::try_from(text.len()) else {
                    return crate::legacy::refuse(bad(String::from("memo past 4 GiB")));
                };
                let bytes = len.to_le_bytes();
                place_bytes(page, cursor, &bytes, table, &column_name)?;
                place_bytes(
                    page,
                    cursor.saturating_add(4),
                    text.as_bytes(),
                    table,
                    &column_name,
                )?;
                Ok(end)
            }
            CellValue::Null => {
                let end = cursor.saturating_add(4);
                if end > page_size {
                    return crate::legacy::refuse(bad(String::from(
                        "null memo runs past the page",
                    )));
                }
                let bytes = 0_u32.to_le_bytes();
                place_bytes(page, cursor, &bytes, table, &column_name)?;
                Ok(end)
            }
            _ => Err(bad(String::from("a memo column needs memo or null"))),
        },
    }
}
