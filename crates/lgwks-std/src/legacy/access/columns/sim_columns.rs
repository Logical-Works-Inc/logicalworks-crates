//! Deterministic simulation of every cell encoder at its page edge.
//!
//! The oracle is the bound each encoder documents, written apart from the
//! code: a cell of `width` bytes at `cursor` fits a page of `page_size` bytes
//! exactly when `cursor + width <= page_size`. Each sim sweeps every cursor of
//! every page size up to [`PAGES`], so an off-by-one in a bound (`>` for `>=`)
//! is caught on the cursor where the two disagree.
//!
//! The fools: the fat-finger fool puts the cell one byte past the edge, the
//! giant-and-empty fool sends a zero-byte page and text wider than its column,
//! and the wrong-order fool hands a column a value of another kind.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `sim_columns_text_fits_exactly_when_its_width_fits` | text: accept iff `cursor + len <= page`, padded with spaces |
//! | `sim_columns_dates_fit_on_the_edge_and_refuse_a_whole_day` | date: 8 bytes, and `millis < MILLIS_PER_DAY` |
//! | `sim_columns_memo_and_null_fit_their_length_prefix` | memo and null: `4 + text` bytes, little-endian length |
//! | `sim_columns_fixed_widths_fit_and_wrong_kinds_refuse_by_name` | integer and currency widths; each mismatch named |

#[cfg(test)]
use super::super::{AccessError, CellValue, ColumnDef, ColumnTag, MILLIS_PER_DAY};
use super::encode_cell;
use crate::legacy::decimal::Decimal;

/// The largest page every sim sweeps, in bytes.
const PAGES: usize = 24;

/// Encodes `cell` into a zeroed page of `page_size` bytes at `cursor`.
fn encode(
    tag: ColumnTag,
    len: u16,
    cell: &CellValue,
    cursor: usize,
    page_size: usize,
) -> (Result<usize, AccessError>, Vec<u8>) {
    let mut page = vec![0_u8; page_size];
    let column = ColumnDef::new("c", tag, len);
    let end = encode_cell(&mut page, cursor, "t", &column, cell, page_size);
    (end, page)
}

/// Whether `result` is a value mismatch whose reason holds `needle`.
fn refused_with(result: &Result<usize, AccessError>, needle: &str) -> bool {
    matches!(result, Err(AccessError::ValueMismatch { why, .. }) if why.contains(needle))
}

/// Checks one cell against the bound at every cursor of every page size,
/// returning how many placements fit, so a sim can assert both sides ran.
fn sweep(
    tag: ColumnTag,
    len: u16,
    cell: &CellValue,
    width: usize,
    past: &str,
    written: &[u8],
) -> usize {
    let mut fitted = 0_usize;
    for page_size in 0..=PAGES {
        for cursor in 0..=page_size.saturating_add(1) {
            let (end, page) = encode(tag, len, cell, cursor, page_size);
            let edge = cursor.saturating_add(width);
            if edge <= page_size {
                assert_eq!(
                    end.as_ref().ok(),
                    Some(&edge),
                    "{tag:?} at {cursor} of {page_size}"
                );
                assert_eq!(page.get(cursor..edge), Some(written), "{tag:?} bytes");
                fitted = fitted.saturating_add(1);
            } else {
                assert!(
                    refused_with(&end, past),
                    "{tag:?} at {cursor} of {page_size}: {end:?}"
                );
            }
        }
    }
    fitted
}

#[test]
fn sim_columns_text_fits_exactly_when_its_width_fits() {
    let text = CellValue::Text(String::from("ab"));
    let fitted = sweep(
        ColumnTag::Text,
        4,
        &text,
        4,
        "text runs past the page",
        b"ab  ",
    );
    assert!(fitted > 0, "the sweep placed text somewhere");
    let wide = CellValue::Text(String::from("abcde"));
    let (over, _) = encode(ColumnTag::Text, 4, &wide, 0, PAGES);
    assert!(refused_with(&over, "text needs 5 bytes past 4"), "{over:?}");
    let (exact, _) = encode(ColumnTag::Text, 5, &wide, 0, PAGES);
    assert_eq!(exact.ok(), Some(5));
}

#[test]
fn sim_columns_dates_fit_on_the_edge_and_refuse_a_whole_day() {
    let last_millis = MILLIS_PER_DAY.saturating_sub(1);
    let last = CellValue::Date {
        days: -2,
        millis: last_millis,
    };
    let mut written = (-2_i32).to_be_bytes().to_vec();
    written.extend_from_slice(&last_millis.to_be_bytes());
    let fitted = sweep(
        ColumnTag::Date,
        0,
        &last,
        8,
        "date runs past the page",
        &written,
    );
    assert!(fitted > 0, "the sweep placed a date somewhere");
    let whole_day = CellValue::Date {
        days: 0,
        millis: MILLIS_PER_DAY,
    };
    let (day, _) = encode(ColumnTag::Date, 0, &whole_day, 0, PAGES);
    assert!(
        refused_with(&day, "date milliseconds past one day"),
        "{day:?}"
    );
}

#[test]
fn sim_columns_memo_and_null_fit_their_length_prefix() {
    let memo = CellValue::Memo(String::from("hey"));
    let written = [3, 0, 0, 0, b'h', b'e', b'y'];
    let fitted = sweep(
        ColumnTag::Memo,
        0,
        &memo,
        7,
        "memo runs past the page",
        &written,
    );
    assert!(fitted > 0, "the sweep placed a memo somewhere");
    let nulls = sweep(
        ColumnTag::Memo,
        0,
        &CellValue::Null,
        4,
        "null memo runs past the page",
        &[0, 0, 0, 0],
    );
    assert!(
        nulls > fitted,
        "a null is shorter than a memo, so it fits more often"
    );
}

#[test]
fn sim_columns_fixed_widths_fit_and_wrong_kinds_refuse_by_name() {
    let int = CellValue::Integer(7);
    let ints = sweep(
        ColumnTag::Integer,
        0,
        &int,
        4,
        "integer runs past the page",
        &7_i32.to_be_bytes(),
    );
    let money = CellValue::Currency(Decimal::new(12_345, 4));
    let cash = sweep(
        ColumnTag::Currency,
        0,
        &money,
        8,
        "currency runs past the page",
        &12_345_i64.to_be_bytes(),
    );
    assert!(
        ints > cash,
        "a 4-byte cell fits more often than an 8-byte one"
    );
    let wrong = CellValue::Integer(1);
    for (tag, needle) in [
        (ColumnTag::Text, "a text column needs text"),
        (ColumnTag::Date, "a date column needs a date"),
        (ColumnTag::Memo, "a memo column needs memo or null"),
        (ColumnTag::Currency, "a currency column needs currency"),
    ] {
        let (refusal, _) = encode(tag, 4, &wrong, 0, PAGES);
        assert!(refused_with(&refusal, needle), "{tag:?}: {refusal:?}");
    }
}
