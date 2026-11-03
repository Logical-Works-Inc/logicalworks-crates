//! `access` reads Jet (`.mdb`) and ACE (`.accdb`) database pages.
//!
//! The page headers, the table catalogue and the rows of plain tables are
//! validated per the Jet/ACE on-disk notes: the `STANDARD JET DB` and
//! `STANDARD ACE DB` magics, the 2048/4096-byte pages, a catalogue page of
//! table and column descriptors, and data pages of fixed rows. Text,
//! integer, currency (exact `Decimal`), date and memo columns decode;
//! encrypted files and unsupported column types are typed errors naming
//! the table and column. Pages live in `page`, the catalogue in `catalog`,
//! cells in `columns`; `rows` reads rows and builds the test images.

/// The table catalogue: names, pages and column descriptors.
mod catalog;
/// Table columns: cell decoding and encoding.
mod columns;
/// Database pages: windows, detection and header writes.
mod page;
/// Table rows: reading data pages and building images.
pub mod rows;

use std::fmt;

use super::decimal::Decimal;
use super::limits::{LimitError, Limits};

/// The Jet magic: `STANDARD JET DB\0` at offset zero.
pub(crate) const JET_MAGIC: &[u8; 20] = b"STANDARD JET DB\0\0\0\0\0";

/// The ACE magic: `STANDARD ACE DB\0` at offset zero.
pub(crate) const ACE_MAGIC: &[u8; 20] = b"STANDARD ACE DB\0\0\0\0\0";

/// Bytes of one page-0 header this codec writes and reads.
const HEADER_LEN: usize = 64;

/// The page kind of a catalogue page.
pub(crate) const PAGE_CATALOG: u8 = 1;

/// The page kind of a data page.
pub(crate) const PAGE_DATA: u8 = 2;

/// The header flag bit marking an encrypted file.
const FLAG_ENCRYPTED: u8 = 0x01;

/// The text column tag.
const TAG_TEXT: u8 = 1;

/// The integer column tag.
const TAG_INTEGER: u8 = 2;

/// The currency column tag.
const TAG_CURRENCY: u8 = 3;

/// The date column tag.
const TAG_DATE: u8 = 4;

/// The memo column tag.
const TAG_MEMO: u8 = 5;

/// Milliseconds in one civil day.
pub(crate) const MILLIS_PER_DAY: u32 = 86_400_000;

/// The database engine a file or image uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DbVersion {
    /// Jet 3: 2048-byte pages, `.mdb`.
    Jet3,
    /// Jet 4: 4096-byte pages, `.mdb`.
    Jet4,
    /// ACE: 4096-byte pages, `.accdb`.
    Ace,
}

impl DbVersion {
    /// The magic at offset zero.
    pub(crate) fn magic(self) -> &'static [u8; 20] {
        match self {
            Self::Jet3 | Self::Jet4 => JET_MAGIC,
            Self::Ace => ACE_MAGIC,
        }
    }

    /// Bytes of one page.
    pub(crate) fn page_size(self) -> usize {
        match self {
            Self::Jet3 => 2048,
            Self::Jet4 | Self::Ace => 4096,
        }
    }

    /// The version byte the header carries.
    pub(crate) fn version_byte(self) -> u8 {
        match self {
            Self::Jet3 => 3,
            Self::Jet4 => 4,
            Self::Ace => 12,
        }
    }

    /// Reads the engine back from a version byte.
    pub(crate) fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            3 => Some(Self::Jet3),
            4 => Some(Self::Jet4),
            12 => Some(Self::Ace),
            _ => None,
        }
    }
}

/// The column storage a definition names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColumnTag {
    /// Fixed-width text, space-padded.
    Text,
    /// A 4-byte big-endian integer.
    Integer,
    /// An 8-byte big-endian ten-thousandths count.
    Currency,
    /// Days and milliseconds: a 4-byte day count plus millisecond part.
    Date,
    /// A length-prefixed UTF-8 value.
    Memo,
}

impl ColumnTag {
    /// The tag byte the catalogue carries.
    pub(crate) fn tag(self) -> u8 {
        match self {
            Self::Text => TAG_TEXT,
            Self::Integer => TAG_INTEGER,
            Self::Currency => TAG_CURRENCY,
            Self::Date => TAG_DATE,
            Self::Memo => TAG_MEMO,
        }
    }

    /// Reads the tag back, or `None` for an unsupported column type.
    pub(crate) fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            TAG_TEXT => Some(Self::Text),
            TAG_INTEGER => Some(Self::Integer),
            TAG_CURRENCY => Some(Self::Currency),
            TAG_DATE => Some(Self::Date),
            TAG_MEMO => Some(Self::Memo),
            _ => None,
        }
    }
}

/// One column definition for [`rows::build_image`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ColumnDef {
    /// The column name.
    name: String,
    /// The column storage.
    tag: ColumnTag,
    /// The text width; zero for fixed types.
    len: u16,
}

impl ColumnDef {
    /// One column: its name, storage and text width.
    #[must_use]
    pub fn new(name: &str, tag: ColumnTag, len: u16) -> Self {
        Self {
            name: String::from(name),
            tag,
            len,
        }
    }
}

/// One decoded cell.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellValue {
    /// A text column, padding trimmed.
    Text(String),
    /// An integer column.
    Integer(i32),
    /// A currency column as an exact decimal of scale 4.
    Currency(Decimal),
    /// A date column: days and the millisecond part of the day.
    Date {
        /// Days since the epoch the file uses.
        days: i32,
        /// Milliseconds into the day, below 86,400,000.
        millis: u32,
    },
    /// A memo column.
    Memo(String),
    /// An empty memo: no bytes stored.
    Null,
}

/// One decoded row: one cell per catalogue column, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Row {
    /// The cells in catalogue column order.
    cells: Vec<CellValue>,
}

impl Row {
    /// The cells in catalogue column order.
    #[must_use]
    pub fn cells(&self) -> &[CellValue] {
        &self.cells
    }
}

/// Why a database image could not be opened or read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccessError {
    /// The first twenty bytes are neither magic.
    BadMagic,
    /// A version byte outside 3, 4 and 12.
    UnknownVersion {
        /// The observed version byte.
        version: u8,
    },
    /// The input ends before the structure does.
    Truncated {
        /// Byte offset where the input ended.
        offset: usize,
        /// Bytes the structure needed.
        need: usize,
    },
    /// A page header or body breaks the layout.
    BadPage {
        /// The page index.
        page: u32,
        /// What broke.
        why: String,
    },
    /// No table carries the name.
    TableMissing {
        /// The requested table.
        table: String,
    },
    /// A column type this codec does not implement.
    UnsupportedType {
        /// The table holding the column.
        table: String,
        /// The column name.
        column: String,
        /// The observed tag byte.
        tag: u8,
    },
    /// The header flag marks an encrypted file.
    Encrypted,
    /// A value outside what its column stores.
    ValueMismatch {
        /// The table holding the column.
        table: String,
        /// The column name.
        column: String,
        /// What did not hold.
        why: String,
    },
    /// The input or output passed a [`Limits`] ceiling.
    Limit(LimitError),
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadMagic => formatter.write_str("database magic mismatch"),
            Self::UnknownVersion { version } => {
                write!(
                    formatter,
                    "database version {version} is not Jet 3, Jet 4 or ACE"
                )
            }
            Self::Truncated { offset, need } => {
                write!(formatter, "database ends at {offset} short of {need} bytes")
            }
            Self::BadPage { page, ref why } => {
                write!(formatter, "database page {page} is broken: {why}")
            }
            Self::TableMissing { ref table } => {
                write!(formatter, "database holds no table {table:?}")
            }
            Self::UnsupportedType {
                ref table,
                ref column,
                tag,
            } => write!(
                formatter,
                "table {table:?} column {column:?} type {tag} is not implemented"
            ),
            Self::Encrypted => formatter.write_str("database is encrypted"),
            Self::ValueMismatch {
                ref table,
                ref column,
                ref why,
            } => write!(
                formatter,
                "table {table:?} column {column:?} value refused: {why}"
            ),
            Self::Limit(ref cause) => {
                write!(formatter, "database limits refused the input: {cause}")
            }
        }
    }
}

crate::legacy::limits::limit_carrying_error!(AccessError);

/// One table: its catalogue entry and its decoded rows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Table {
    /// The table name.
    name: String,
    /// The catalogue columns in order.
    columns: Vec<catalog::Column>,
    /// The decoded rows in file order.
    rows: Vec<Row>,
}

/// An opened database: the catalogue and the rows of every plain table.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AccessDb {
    /// The engine the file declares.
    version: DbVersion,
    /// The tables in catalogue order.
    tables: Vec<Table>,
}

impl AccessDb {
    /// Opens and validates a database image.
    ///
    /// # Errors
    ///
    /// [`AccessError`] for a bad magic, version, truncation, broken page,
    /// unsupported type or encrypted file, or [`AccessError::Limit`] past a
    /// ceiling.
    pub fn open(bytes: &[u8], limits: &Limits) -> Result<Self, AccessError> {
        limits.check_bytes(bytes.len())?;
        if bytes.len() < HEADER_LEN {
            let refusal = Err(AccessError::Truncated {
                offset: bytes.len(),
                need: HEADER_LEN,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a file shorter than its header");
            return refusal;
        }
        let version = page::detect_version(bytes)?;
        let page_size = version.page_size();
        let Some(flags) = bytes.get(22).copied() else {
            let refusal = Err(AccessError::Truncated {
                offset: 22,
                need: 23,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a header with no flags");
            return refusal;
        };
        if flags & FLAG_ENCRYPTED != 0 {
            let refusal = Err(AccessError::Encrypted);
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing an encrypted file");
            return refusal;
        }
        let Some(page_count) = crate::legacy::get_u32_le(bytes, 24) else {
            let refusal = Err(AccessError::Truncated {
                offset: 24,
                need: 28,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a header with no page count");
            return refusal;
        };
        let Some(catalog_page) = crate::legacy::get_u32_le(bytes, 28) else {
            let refusal = Err(AccessError::Truncated {
                offset: 28,
                need: 32,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a header with no catalogue page");
            return refusal;
        };
        let page_total = match usize::try_from(page_count) {
            Ok(drawn) => drawn,
            Err(_) => {
                let refusal = Err(AccessError::Truncated {
                    offset: 24,
                    need: 28,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("access: refusing a page count past the address width");
                return refusal;
            }
        };
        limits.check_pages(page_total)?;
        if bytes.len() < page_total.saturating_mul(page_size) {
            let refusal = Err(AccessError::Truncated {
                offset: bytes.len(),
                need: page_total.saturating_mul(page_size),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a file shorter than its page count");
            return refusal;
        }
        let tables = catalog::read_catalog(bytes, catalog_page, page_size, limits)?;
        let mut decoded: Vec<Table> = Vec::with_capacity(tables.len());
        for (index, (name, columns, pages)) in tables.into_iter().enumerate() {
            let Ok(table_index) = u32::try_from(index) else {
                let refusal = Err(AccessError::BadPage {
                    page: 0,
                    why: String::from("table ordinal past 32 bits"),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("access: refusing a table ordinal past 32 bits");
                return refusal;
            };
            let rows = rows::read_table_rows(
                bytes,
                &name,
                &columns,
                &pages,
                page_size,
                table_index,
                limits,
            )?;
            decoded.push(Table {
                name,
                columns,
                rows,
            });
        }
        Ok(Self {
            version,
            tables: decoded,
        })
    }

    /// The engine the file declares.
    #[must_use]
    pub const fn version(&self) -> DbVersion {
        self.version
    }

    /// The table names in catalogue order.
    #[must_use]
    pub fn tables(&self) -> Vec<String> {
        self.tables.iter().map(|table| table.name.clone()).collect()
    }

    /// The rows of one table in file order.
    ///
    /// # Errors
    ///
    /// [`AccessError::TableMissing`] for an unknown table.
    pub fn rows(&self, table: &str) -> Result<&[Row], AccessError> {
        let Some(found) = self.tables.iter().find(|entry| entry.name == table) else {
            let refusal = Err(AccessError::TableMissing {
                table: String::from(table),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a missing table");
            return refusal;
        };
        Ok(&found.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{AccessDb, CellValue, ColumnDef, ColumnTag, DbVersion, rows};
    use crate::legacy::decimal::Decimal;
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), super::AccessError>;

    /// The small customer table the unit tests build.
    const COLUMNS: [(&str, ColumnTag, u16); 4] = [
        ("NAME", ColumnTag::Text, 10),
        ("AGE", ColumnTag::Integer, 0),
        ("BAL", ColumnTag::Currency, 0),
        ("NOTE", ColumnTag::Memo, 0),
    ];

    #[test]
    fn a_plain_table_round_trips() -> UnitResult {
        let defs: Vec<ColumnDef> = COLUMNS
            .iter()
            .map(|&(name, tag, len)| ColumnDef::new(name, tag, len))
            .collect();
        let rows = vec![vec![
            CellValue::Text(String::from("ADA")),
            CellValue::Integer(36),
            CellValue::Currency(Decimal::new(123_400, 4)),
            CellValue::Memo(String::from("hi")),
        ]];
        let image = rows::build_image(DbVersion::Jet4, &[("CUSTOMER", &defs, &rows)], &BOUNDS)?;
        let db = AccessDb::open(&image, &BOUNDS)?;
        assert_eq!(
            db.tables(),
            vec![String::from("CUSTOMER")],
            "the catalogue must list the table"
        );
        let read = db.rows("CUSTOMER")?;
        assert_eq!(read.len(), 1, "one row must read back");
        let Some(first) = read.first() else {
            return Err(super::AccessError::TableMissing {
                table: String::from("CUSTOMER"),
            });
        };
        assert_eq!(
            first.cells(),
            &[
                CellValue::Text(String::from("ADA")),
                CellValue::Integer(36),
                CellValue::Currency(Decimal::new(123_400, 4)),
                CellValue::Memo(String::from("hi")),
            ],
            "the row must read back exactly"
        );
        Ok(())
    }

    #[test]
    fn an_unknown_table_names_itself() -> UnitResult {
        let image = rows::build_image(DbVersion::Ace, &[], &BOUNDS)?;
        let db = AccessDb::open(&image, &BOUNDS)?;
        assert!(
            matches!(
                db.rows("MISSING"),
                Err(super::AccessError::TableMissing { .. })
            ),
            "a missing table must name itself"
        );
        Ok(())
    }

    #[test]
    fn an_encrypted_flag_is_refused() -> UnitResult {
        let mut image = rows::build_image(DbVersion::Jet3, &[], &BOUNDS)?;
        if let Some(slot) = image.get_mut(22) {
            *slot = 0x01;
        }
        assert!(
            matches!(
                AccessDb::open(&image, &BOUNDS),
                Err(super::AccessError::Encrypted)
            ),
            "the encrypted flag must refuse the file"
        );
        Ok(())
    }
}
