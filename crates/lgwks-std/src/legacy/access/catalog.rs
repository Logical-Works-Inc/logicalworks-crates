//! The table catalogue: names, pages and column descriptors.
//!
//! [`read_catalog`] parses the catalogue page into table names, data pages
//! and columns; unknown column tags are refused as
//! [`AccessError::UnsupportedType`] naming the table and column.
//!
//! [`AccessError::UnsupportedType`]: super::AccessError::UnsupportedType

use super::super::limits::Limits;
use super::{AccessError, ColumnTag};
use crate::legacy::access::page::page_window;
use crate::legacy::{get_u16_le, get_u32_le};

/// One catalogue column: name, storage and text width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Column {
    /// The column name.
    pub(crate) name: String,
    /// The column storage.
    pub(crate) tag: ColumnTag,
    /// The text width; zero for fixed types.
    pub(crate) len: usize,
}

/// One catalogued table: its name, columns and data pages.
pub(crate) type CatalogTable = (String, Vec<Column>, Vec<u32>);

/// One parsed catalogue entry: name, table page, columns, data pages and the
/// cursor just past it.
type CatalogEntry = (String, u32, Vec<Column>, Vec<u32>, usize);

/// Reads the catalogue page: table names, pages and column descriptors.
pub(crate) fn read_catalog(
    bytes: &[u8],
    catalog_page: u32,
    page_size: usize,
    limits: &Limits,
) -> Result<Vec<CatalogTable>, AccessError> {
    let window = page_window(bytes, catalog_page, page_size)?;
    let Some(kind) = window.first().copied() else {
        return crate::legacy::refuse(AccessError::BadPage {
            page: catalog_page,
            why: String::from("catalogue page is empty"),
        });
    };
    if kind != super::PAGE_CATALOG {
        return crate::legacy::refuse(AccessError::BadPage {
            page: catalog_page,
            why: String::from("catalogue page has the wrong kind"),
        });
    }
    let Some(count) = get_u16_le(window, 1) else {
        return crate::legacy::refuse(AccessError::BadPage {
            page: catalog_page,
            why: String::from("catalogue page has no count"),
        });
    };
    limits.check_records(usize::from(count))?;
    let mut cursor: usize = 3;
    let mut tables = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let (name, _table_page, columns, pages, next) =
            read_catalog_entry(window, cursor, catalog_page)?;
        cursor = next;
        tables.push((name, columns, pages));
    }
    Ok(tables)
}

/// Reads one catalogue entry: name, table page, columns and data pages.
fn read_catalog_entry(
    window: &[u8],
    cursor: usize,
    catalog_page: u32,
) -> Result<CatalogEntry, AccessError> {
    let bad = |why: &str| AccessError::BadPage {
        page: catalog_page,
        why: String::from(why),
    };
    let Some(&name_len) = window.get(cursor) else {
        return crate::legacy::refuse(bad("catalogue entry has no name length"));
    };
    let name_end = cursor
        .saturating_add(1)
        .saturating_add(usize::from(name_len));
    let Some(name_window) = window.get(cursor.saturating_add(1)..name_end) else {
        return crate::legacy::refuse(bad("catalogue entry has no name"));
    };
    let name = match String::from_utf8(name_window.to_vec()) {
        Ok(drawn) => drawn,
        Err(_) => return crate::legacy::refuse(bad("catalogue table name is not UTF-8")),
    };
    let Some(table_page) = get_u32_le(window, name_end) else {
        return crate::legacy::refuse(bad("catalogue entry has no table page"));
    };
    let col_at = name_end.saturating_add(4);
    let Some(col_count) = get_u16_le(window, col_at) else {
        return crate::legacy::refuse(bad("catalogue entry has no column count"));
    };
    let mut at = col_at.saturating_add(2);
    let mut columns = Vec::with_capacity(usize::from(col_count));
    for _ in 0..col_count {
        let Some(&tag) = window.get(at) else {
            return crate::legacy::refuse(bad("catalogue column has no tag"));
        };
        let Some(len) = get_u16_le(window, at.saturating_add(1)) else {
            return crate::legacy::refuse(bad("catalogue column has no width"));
        };
        let Some(&column_name_len) = window.get(at.saturating_add(3)) else {
            return crate::legacy::refuse(bad("catalogue column has no name length"));
        };
        let column_end = at
            .saturating_add(4)
            .saturating_add(usize::from(column_name_len));
        let Some(column_window) = window.get(at.saturating_add(4)..column_end) else {
            return crate::legacy::refuse(bad("catalogue column has no name"));
        };
        let column_name = match String::from_utf8(column_window.to_vec()) {
            Ok(drawn) => drawn,
            Err(_) => return crate::legacy::refuse(bad("catalogue column name is not UTF-8")),
        };
        let Some(storage) = ColumnTag::from_tag(tag) else {
            return crate::legacy::refuse(AccessError::UnsupportedType {
                table: name,
                column: column_name,
                tag,
            });
        };
        columns.push(Column {
            name: column_name,
            tag: storage,
            len: usize::from(len),
        });
        at = column_end;
    }
    let Some(&page_total) = window.get(at) else {
        return crate::legacy::refuse(bad("catalogue entry has no page total"));
    };
    at = at.saturating_add(1);
    let mut pages = Vec::with_capacity(usize::from(page_total));
    for _ in 0..page_total {
        let Some(page) = get_u32_le(window, at) else {
            return crate::legacy::refuse(bad("catalogue entry has no data page"));
        };
        pages.push(page);
        at = at.saturating_add(4);
    }
    Ok((name, table_page, columns, pages, at))
}
