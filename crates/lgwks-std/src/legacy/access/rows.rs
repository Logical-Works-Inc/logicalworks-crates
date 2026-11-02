//! Table rows: reading data pages and building images.
//!
//! [`read_table_rows`] walks a table's data pages into decoded [`Row`]
//! items; [`build_image`] lays out the header, catalogue and data pages the
//! round-trip tests read back.
//!
//! [`Row`]: super::Row

use super::super::limits::Limits;
use super::catalog::Column;
use super::columns::encode_cell;
use super::page::{page_window, write_header_counts, write_u16_at};
use super::{AccessError, CellValue, ColumnDef, DbVersion, Row};
use crate::legacy::{get_u16_le, get_u32_le, push_u16_le, push_u32_le};

/// Reads every row of one table across its data pages.
pub(crate) fn read_table_rows(
    bytes: &[u8],
    table: &str,
    columns: &[Column],
    pages: &[u32],
    page_size: usize,
    table_index: u32,
    limits: &Limits,
) -> Result<Vec<Row>, AccessError> {
    limits.check_pages(pages.len())?;
    let mut rows: Vec<Row> = Vec::new();
    for page in pages.iter() {
        let window = page_window(bytes, *page, page_size)?;
        let Some(kind) = window.first().copied() else {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page is empty"),
            });
        };
        if kind != super::PAGE_DATA {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page has the wrong kind"),
            });
        }
        let Some(seen_table) = get_u32_le(window, 1) else {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page has no table index"),
            });
        };
        if seen_table != table_index {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page names the wrong table"),
            });
        }
        let Some(seen_columns) = get_u16_le(window, 5) else {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page has no column count"),
            });
        };
        if usize::from(seen_columns) != columns.len() {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page column count disagrees with the catalogue"),
            });
        }
        let Some(row_total) = get_u16_le(window, 7) else {
            return crate::legacy::refuse(AccessError::BadPage {
                page: *page,
                why: String::from("data page has no row count"),
            });
        };
        limits.check_records(rows.len().saturating_add(usize::from(row_total)))?;
        let mut cursor: usize = 9;
        for _ in 0..row_total {
            let (row, next) = read_row(window, cursor, table, columns)?;
            rows.push(row);
            cursor = next;
        }
    }
    Ok(rows)
}

/// Reads one row: one cell per catalogue column.
fn read_row(
    window: &[u8],
    cursor: usize,
    table: &str,
    columns: &[Column],
) -> Result<(Row, usize), AccessError> {
    let mut at = cursor;
    let mut cells = Vec::with_capacity(columns.len());
    for column in columns.iter() {
        let (cell, next) = super::columns::read_cell(window, at, table, column)?;
        cells.push(cell);
        at = next;
    }
    Ok((Row { cells }, at))
}

/// An empty data page for table `ordinal`: kind, table index, zero counts.
fn empty_data_page(ordinal: u32, page_size: usize) -> Vec<u8> {
    let mut page = vec![0_u8; page_size];
    if let Some(slot) = page.first_mut() {
        *slot = super::PAGE_DATA;
    }
    if let Some(window) = page.get_mut(1..5) {
        window.copy_from_slice(&ordinal.to_le_bytes());
    }
    page
}

/// Lays a table's rows over as many data pages as they need, never
/// splitting a row: a row that does not fit starts the next page, and one
/// that does not fit an empty page is refused by the cell that overflows.
fn write_data_pages(
    table: &str,
    ordinal: u32,
    columns: &[ColumnDef],
    rows: &[Vec<CellValue>],
    page_size: usize,
) -> Result<Vec<Vec<u8>>, AccessError> {
    const FIRST_ROW: usize = 9;
    let overflow = || AccessError::ValueMismatch {
        table: String::from(table),
        column: String::from("<row>"),
        why: String::from("row runs past the page"),
    };
    let mut pages: Vec<Vec<u8>> = Vec::new();
    let mut page = empty_data_page(ordinal, page_size);
    let mut scratch = vec![0_u8; page_size];
    let mut cursor = FIRST_ROW;
    let mut on_page: usize = 0;
    for row in rows {
        if row.len() != columns.len() {
            return crate::legacy::refuse(AccessError::ValueMismatch {
                table: String::from(table),
                column: String::from("<row>"),
                why: format!(
                    "row holds {} cells for {} columns",
                    row.len(),
                    columns.len()
                ),
            });
        }
        let mut end = FIRST_ROW;
        for (column, cell) in columns.iter().zip(row.iter()) {
            end = encode_cell(&mut scratch, end, table, column, cell, page_size)?;
        }
        let encoded = scratch.get(FIRST_ROW..end).ok_or_else(overflow)?;
        if cursor.saturating_add(encoded.len()) > page_size {
            write_u16_at(&mut page, 5, columns.len())?;
            write_u16_at(&mut page, 7, on_page)?;
            pages.push(core::mem::replace(
                &mut page,
                empty_data_page(ordinal, page_size),
            ));
            cursor = FIRST_ROW;
            on_page = 0;
        }
        let row_end = cursor.saturating_add(encoded.len());
        page.get_mut(cursor..row_end)
            .ok_or_else(overflow)?
            .copy_from_slice(encoded);
        cursor = row_end;
        on_page = on_page.saturating_add(1);
    }
    write_u16_at(&mut page, 5, columns.len())?;
    write_u16_at(&mut page, 7, on_page)?;
    pages.push(page);
    Ok(pages)
}

/// One table to write: its name, columns and rows.
pub type TableSpec<'spec> = (&'spec str, &'spec [ColumnDef], &'spec [Vec<CellValue>]);

/// Builds a database image holding the given tables and rows.
///
/// One header page, one catalogue page, and as many whole-row data pages
/// per table as its rows fill, up to the 255 a catalogue entry lists. Memo
/// cells with no bytes encode as nulls.
///
/// # Errors
///
/// [`AccessError::ValueMismatch`] for a value its column cannot store or a
/// table past 255 pages, or [`AccessError::Limit`] past a ceiling.
pub fn build_image(
    version: DbVersion,
    tables: &[TableSpec<'_>],
    limits: &Limits,
) -> Result<Vec<u8>, AccessError> {
    let page_size = version.page_size();
    limits.check_records(tables.len())?;
    let mut header = vec![0_u8; page_size];
    if let Some(window) = header.get_mut(..20) {
        window.copy_from_slice(version.magic());
    }
    if let Some(slot) = header.get_mut(20) {
        *slot = version.version_byte();
    }
    let mut catalog_body: Vec<u8> = Vec::new();
    let mut data_pages: Vec<Vec<u8>> = Vec::new();
    let mut page_index: u32 = 2;
    for (table_ordinal, &(table_name, columns, rows)) in tables.iter().enumerate() {
        limits.check_records(rows.len())?;
        let Ok(table_ordinal) = u32::try_from(table_ordinal) else {
            return crate::legacy::refuse(AccessError::BadPage {
                page: 0,
                why: String::from("table ordinal past 32 bits"),
            });
        };
        let table_pages = write_data_pages(table_name, table_ordinal, columns, rows, page_size)?;
        let Ok(page_total) = u8::try_from(table_pages.len()) else {
            return crate::legacy::refuse(AccessError::ValueMismatch {
                table: String::from(table_name),
                column: String::from("<table>"),
                why: format!(
                    "{} data pages past the 255 a catalogue entry lists",
                    table_pages.len()
                ),
            });
        };
        limits.check_pages(data_pages.len().saturating_add(table_pages.len()))?;
        let table_page = page_index;
        let mut listed: Vec<u32> = Vec::with_capacity(table_pages.len());
        for page in table_pages {
            listed.push(page_index);
            page_index = page_index.saturating_add(1);
            data_pages.push(page);
        }
        catalog_body.push(match u8::try_from(table_name.len()) {
            Ok(drawn) => drawn,
            Err(_) => {
                return crate::legacy::refuse(AccessError::ValueMismatch {
                    table: String::from(table_name),
                    column: String::from("<table>"),
                    why: String::from("table name past 255 bytes"),
                });
            }
        });
        catalog_body.extend_from_slice(table_name.as_bytes());
        push_u32_le(&mut catalog_body, table_page);
        let column_total = match u16::try_from(columns.len()) {
            Ok(drawn) => drawn,
            Err(_) => {
                return crate::legacy::refuse(AccessError::BadPage {
                    page: 1,
                    why: String::from("column total past 16 bits"),
                });
            }
        };
        push_u16_le(&mut catalog_body, column_total);
        for column in columns.iter() {
            catalog_body.push(column.tag.tag());
            push_u16_le(&mut catalog_body, column.len);
            catalog_body.push(match u8::try_from(column.name.len()) {
                Ok(drawn) => drawn,
                Err(_) => {
                    return crate::legacy::refuse(AccessError::ValueMismatch {
                        table: String::from(table_name),
                        column: column.name.clone(),
                        why: String::from("column name past 255 bytes"),
                    });
                }
            });
            catalog_body.extend_from_slice(column.name.as_bytes());
        }
        catalog_body.push(page_total);
        for page in listed {
            push_u32_le(&mut catalog_body, page);
        }
    }
    let table_total = match u16::try_from(tables.len()) {
        Ok(drawn) => drawn,
        Err(_) => {
            return crate::legacy::refuse(AccessError::BadPage {
                page: 1,
                why: String::from("table total past 16 bits"),
            });
        }
    };
    let mut catalog_page = vec![0_u8; page_size];
    if let Some(slot) = catalog_page.first_mut() {
        *slot = super::PAGE_CATALOG;
    }
    let mut head: Vec<u8> = Vec::new();
    push_u16_le(&mut head, table_total);
    head.extend_from_slice(&catalog_body);
    let body_len = head.len();
    if body_len.saturating_add(1) > page_size {
        return crate::legacy::refuse(AccessError::BadPage {
            page: 1,
            why: String::from("catalogue past one page"),
        });
    }
    if let Some(window) = catalog_page.get_mut(1..1_usize.saturating_add(body_len)) {
        window.copy_from_slice(&head);
    }
    let mut image: Vec<u8> = Vec::new();
    image.extend_from_slice(&header);
    image.extend_from_slice(&catalog_page);
    for page in data_pages.iter() {
        image.extend_from_slice(page);
    }
    let page_total = image.len().div_ceil(page_size);
    write_header_counts(&mut image, version, page_total, limits)?;
    limits.check_bytes(image.len())?;
    Ok(image)
}
