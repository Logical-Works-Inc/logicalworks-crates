//! The copybook record model: fields, values, dates and located errors.
//!
//! The public surface of the copybook codec lives here: [`LayoutField`]
//! describes one laid-out field, [`FieldValue`] carries one decoded value,
//! [`DateValue`] keeps raw bytes beside a parsed date, and
//! [`CopybookError`] locates every refusal.

use std::fmt;

use super::super::decimal::Decimal;
use super::super::limits::LimitError;
use super::Slot;
use super::SlotKind;

/// A date field keeps its raw bytes beside the parsed value.
///
/// COBOL has no date type: a field whose name carries `DATE` and whose
/// picture is eight unsigned zoned digits is read as a calendar date, and the
/// eight bytes stay on the value so a re-encode reproduces them exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DateValue {
    /// The eight raw record bytes the date was read from.
    raw: Vec<u8>,
    /// The calendar year, four digits.
    year: u16,
    /// The calendar month, 1-12.
    month: u8,
    /// The calendar day, 1-31.
    day: u8,
}

impl DateValue {
    /// Assembles a date from raw bytes and a parsed year, month and day.
    pub(crate) fn assemble(raw: Vec<u8>, year: u16, month: u8, day: u8) -> Self {
        Self {
            raw,
            year,
            month,
            day,
        }
    }

    /// The eight raw record bytes the date was read from.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The calendar year, four digits.
    #[must_use]
    pub const fn year(&self) -> u16 {
        self.year
    }

    /// The calendar month, 1-12.
    #[must_use]
    pub const fn month(&self) -> u8 {
        self.month
    }

    /// The calendar day, 1-31.
    #[must_use]
    pub const fn day(&self) -> u8 {
        self.day
    }
}

/// One decoded record value, aligned with its [`LayoutField`].
///
/// [`LayoutField`]: super::LayoutField
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldValue {
    /// An `X` text field.
    Text(String),
    /// A numeric field as an exact decimal.
    Number(Decimal),
    /// A `DATE` field with raw bytes beside the parsed value.
    Date(DateValue),
    /// The occurrences of an `OCCURS` field, in order.
    Group(Vec<FieldValue>),
}

/// One laid-out field of a record layout.
///
/// Fixed `OCCURS` copies are expanded inline, so each copy is its own field;
/// a `DEPENDING ON` field stays one field whose value is a [`FieldValue::Group`].
///
/// [`FieldValue::Group`]: FieldValue::Group
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LayoutField {
    /// The field name, folded to uppercase.
    name: String,
    /// Byte offset in the record.
    offset: usize,
    /// Bytes the field holds at most.
    len: usize,
    /// The storage shape.
    kind: SlotKind,
    /// Implied decimal places.
    scale: u32,
    /// Repetition floor.
    min_occurs: usize,
    /// Repetition ceiling.
    max_occurs: usize,
    /// The count field for `DEPENDING ON`; `None` for a fixed layout.
    depending_on: Option<String>,
    /// Bytes one occurrence holds.
    stride: usize,
    /// The expanded children for a dynamic group; empty otherwise.
    sub: Vec<LayoutField>,
}

impl LayoutField {
    /// Builds a public field from one laid-out slot and its children.
    pub(crate) fn from_slot(slot: &Slot, sub: Vec<LayoutField>) -> Self {
        Self {
            name: slot.name.clone(),
            offset: slot.offset,
            len: slot.len,
            kind: slot.kind,
            scale: slot.scale,
            min_occurs: slot.min_occurs,
            max_occurs: slot.max_occurs,
            depending_on: slot.depending_on.clone(),
            stride: slot.stride,
            sub,
        }
    }

    /// The field name, folded to uppercase.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Byte offset in the record.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Bytes the field holds at most.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the field holds no bytes: a group whose ceiling is zero.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The storage label: `X`, `9`, `S9`, `COMP-3` or `COMP-4`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self.kind {
            SlotKind::Text => "X",
            SlotKind::Zoned { signed: false, .. } => "9",
            SlotKind::Zoned { signed: true, .. } => "S9",
            SlotKind::Packed { .. } => "COMP-3",
            SlotKind::Binary { .. } => "COMP-4",
        }
    }

    /// Implied decimal places.
    #[must_use]
    pub const fn scale(&self) -> u32 {
        self.scale
    }

    /// Whether the count comes from a `DEPENDING ON` field.
    #[must_use]
    pub const fn is_dynamic(&self) -> bool {
        self.depending_on.is_some()
    }

    /// Repetition ceiling: 1, the fixed count, or the depending ceiling.
    #[must_use]
    pub const fn max_occurs(&self) -> usize {
        self.max_occurs
    }

    /// The expanded children for a dynamic group; empty otherwise.
    #[must_use]
    pub fn children(&self) -> &[LayoutField] {
        &self.sub
    }

    /// The storage shape, for the crate's reader and writer.
    #[must_use]
    pub(crate) const fn shape(&self) -> SlotKind {
        self.kind
    }

    /// Bytes one occurrence holds, for the crate's reader and writer.
    #[must_use]
    pub(crate) const fn stride(&self) -> usize {
        self.stride
    }

    /// Repetition floor, for the crate's reader and writer.
    #[must_use]
    pub(crate) const fn min_occurs(&self) -> usize {
        self.min_occurs
    }

    /// The count field for `DEPENDING ON`, for the crate's reader and writer.
    #[must_use]
    pub(crate) fn holder(&self) -> Option<&str> {
        self.depending_on.as_deref()
    }

    /// Builds a template leaf for a dynamic group.
    pub(crate) fn template_leaf(
        name: String,
        offset: usize,
        stride: usize,
        kind: SlotKind,
        scale: u32,
    ) -> Self {
        Self {
            name,
            offset,
            len: stride,
            kind,
            scale,
            min_occurs: 1,
            max_occurs: 1,
            depending_on: None,
            stride,
            sub: Vec::new(),
        }
    }
}

/// Why a copybook could not be parsed, or a record decoded or encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CopybookError {
    /// A level outside 01-49 that is neither 66 nor 88.
    BadLevel {
        /// One-based source line.
        line: usize,
        /// The offending level text.
        text: String,
    },
    /// A 66 `RENAMES` entry, which this parser refuses by design.
    RenamesRefused {
        /// One-based source line.
        line: usize,
        /// The entry name.
        name: String,
    },
    /// A clause outside the admitted set.
    UnknownClause {
        /// One-based source line.
        line: usize,
        /// The offending clause text.
        text: String,
    },
    /// A `PIC` string outside the admitted pictures.
    BadPic {
        /// One-based source line.
        line: usize,
        /// The field name.
        field: String,
        /// The offending picture text.
        text: String,
    },
    /// An `OCCURS` clause outside the admitted counts.
    BadOccurs {
        /// One-based source line.
        line: usize,
        /// The field name.
        field: String,
        /// The offending clause text.
        text: String,
    },
    /// A `REDEFINES` naming no earlier field.
    UnknownRedefines {
        /// One-based source line.
        line: usize,
        /// The field name.
        field: String,
        /// The missing target.
        name: String,
    },
    /// A name resolving to more than one earlier field.
    AmbiguousName {
        /// One-based source line.
        line: usize,
        /// The ambiguous name.
        name: String,
    },
    /// A `DEPENDING ON` naming no earlier field.
    DependsOnMissing {
        /// One-based source line.
        line: usize,
        /// The field name.
        field: String,
        /// The missing count field.
        name: String,
    },
    /// A `DEPENDING ON` naming a non-numeric field.
    DependsOnNotNumeric {
        /// One-based source line.
        line: usize,
        /// The field name.
        field: String,
        /// The count field that is not numeric.
        name: String,
    },
    /// A dynamic count outside its floor and ceiling.
    CountOutOfRange {
        /// The field name.
        field: String,
        /// The observed count.
        count: usize,
        /// The admitted floor.
        min: usize,
        /// The admitted ceiling.
        max: usize,
    },
    /// A record shorter or longer than the layout.
    LengthMismatch {
        /// Bytes the layout holds.
        need: usize,
        /// Bytes the record holds.
        have: usize,
    },
    /// A field slice past the record end.
    TruncatedRecord {
        /// The field name.
        field: String,
        /// Bytes the field needs.
        need: usize,
        /// Bytes the record holds.
        have: usize,
    },
    /// A value outside what its field stores.
    ValueMismatch {
        /// The field name.
        field: String,
        /// What did not hold.
        why: String,
    },
    /// A `DATE` field whose digits are not a calendar date.
    InvalidDate {
        /// The field name.
        field: String,
        /// The eight digits that failed validation.
        text: String,
    },
    /// The input passed a [`Limits`] ceiling.
    ///
    /// [`Limits`]: super::super::limits::Limits
    Limit(LimitError),
}

impl fmt::Display for CopybookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadLevel { line, ref text } => {
                write!(
                    formatter,
                    "copybook line {line}: level {text:?} is not 01-49, 66 or 88"
                )
            }
            Self::RenamesRefused { line, ref name } => {
                write!(
                    formatter,
                    "copybook line {line}: 66 RENAMES {name:?} is refused"
                )
            }
            Self::UnknownClause { line, ref text } => {
                write!(
                    formatter,
                    "copybook line {line}: clause {text:?} is not admitted"
                )
            }
            Self::BadPic {
                line,
                ref field,
                ref text,
            } => write!(
                formatter,
                "copybook line {line}: field {field:?} picture {text:?} is not admitted"
            ),
            Self::BadOccurs {
                line,
                ref field,
                ref text,
            } => write!(
                formatter,
                "copybook line {line}: field {field:?} OCCURS {text:?} is not admitted"
            ),
            Self::UnknownRedefines {
                line,
                ref field,
                ref name,
            } => write!(
                formatter,
                "copybook line {line}: field {field:?} redefines unknown {name:?}"
            ),
            Self::AmbiguousName { line, ref name } => {
                write!(
                    formatter,
                    "copybook line {line}: name {name:?} resolves twice"
                )
            }
            Self::DependsOnMissing {
                line,
                ref field,
                ref name,
            } => write!(
                formatter,
                "copybook line {line}: field {field:?} depends on missing {name:?}"
            ),
            Self::DependsOnNotNumeric {
                line,
                ref field,
                ref name,
            } => write!(
                formatter,
                "copybook line {line}: field {field:?} depends on non-numeric {name:?}"
            ),
            Self::CountOutOfRange {
                ref field,
                count,
                min,
                max,
            } => write!(
                formatter,
                "field {field:?} count {count} is past {min} TO {max}"
            ),
            Self::LengthMismatch { need, have } => {
                write!(
                    formatter,
                    "record holds {have} bytes for a {need}-byte layout"
                )
            }
            Self::TruncatedRecord {
                ref field,
                need,
                have,
            } => {
                write!(
                    formatter,
                    "field {field:?} needs {need} bytes of a {have}-byte record"
                )
            }
            Self::ValueMismatch { ref field, ref why } => {
                write!(formatter, "field {field:?} value refused: {why}")
            }
            Self::InvalidDate {
                ref field,
                ref text,
            } => {
                write!(
                    formatter,
                    "field {field:?} date {text:?} is not a calendar date"
                )
            }
            Self::Limit(ref cause) => {
                write!(formatter, "copybook limits refused the input: {cause}")
            }
        }
    }
}

crate::legacy::limits::limit_carrying_error!(CopybookError);

impl From<super::super::decimal::DecimalError> for CopybookError {
    /// Carries a decimal refusal into a value mismatch naming no field.
    fn from(cause: super::super::decimal::DecimalError) -> Self {
        Self::ValueMismatch {
            field: String::from("<numeric>"),
            why: format!("{cause}"),
        }
    }
}
