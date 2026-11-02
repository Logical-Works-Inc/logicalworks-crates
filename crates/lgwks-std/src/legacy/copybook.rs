//! `copybook` parses COBOL copybooks into record layouts and reads fixed-width records.
//!
//! Levels 01-49 form groups and elementary items; 66 `RENAMES` is refused
//! with a named diagnostic, 88 condition names are kept, and the admitted
//! pictures are `X`, `9`, `S9`, `V`, `COMP`, `COMP-3`, `COMP-4` and `BINARY`
//! with `OCCURS n`, `OCCURS m TO n DEPENDING ON` and `REDEFINES` (IBM
//! Enterprise COBOL Language Reference, record description entries). A
//! parsed [`RecordLayout`] decodes a fixed-width record into [`FieldValue`]
//! items and encodes them back, so `decode` then `encode` returns the same
//! bytes. A field whose name carries `DATE` and whose picture is eight
//! unsigned zoned digits decodes as [`DateValue`], which keeps the raw bytes
//! beside the parsed year, month and day. Any other clause is a located
//! [`CopybookError`], never a guess.
//!
//! The record model — fields, values, dates and errors — lives in [`model`];
//! the clause grammar in `grammar`; the level tree and layout in `layout`;
//! decoding in `reader`; encoding in `writer`.
//!
//! [`FieldValue`]: model::FieldValue
//! [`DateValue`]: model::DateValue
//! [`CopybookError`]: model::CopybookError

/// The clause grammar: items, `OCCURS` and `PIC`.
mod grammar;
/// The level tree: building, expanding and laying out.
mod layout;
/// The record model: fields, values, dates and located errors.
mod model;
/// Fixed-width record decoding: bytes into values.
mod reader;
/// Fixed-width record encoding: values into bytes.
mod writer;

pub use self::model::{CopybookError, DateValue, FieldValue, LayoutField};
use super::ebcdic::CodePage;
use super::limits::Limits;

/// The space every EBCDIC page here spells as 0x40: the fill between fields.
const EBCDIC_SPACE: u8 = 0x40;

/// How a numeric picture is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Usage {
    /// Plain display bytes: text or zoned digits.
    Display,
    /// Packed `COMP-3` nibbles with a sign.
    Packed,
    /// Big-endian binary `COMP`/`COMP-4`/`BINARY` integer.
    Binary,
}

/// A parsed `PIC` clause: what the field holds and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Picture {
    /// Text length for an `X` picture; `None` for a numeric picture.
    text_len: Option<usize>,
    /// Digit count for a numeric picture; zero for text.
    digits: usize,
    /// Implied decimal places after `V`.
    scale: u32,
    /// Whether an `S` signed the picture.
    signed: bool,
    /// The storage the usage clause names.
    usage: Usage,
}

/// A parsed `OCCURS` clause: fixed or `DEPENDING ON` repetition.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Occurs {
    /// Repetitions when fixed; the floor when depending.
    min: usize,
    /// Repetitions when fixed; the ceiling when depending.
    max: usize,
    /// The count field for `DEPENDING ON`; `None` for a fixed count.
    depending_on: Option<String>,
}

/// One parsed copybook line before the level tree is built.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    /// One-based source line, for located diagnostics.
    line_no: usize,
    /// The declared level, 1-49.
    level: u8,
    /// The field name, folded to uppercase.
    name: String,
    /// The parsed picture; `None` for a group header.
    picture: Option<Picture>,
    /// The parsed repetition; `None` for a single field.
    occurs: Option<Occurs>,
    /// The `REDEFINES` target; `None` for a fresh offset.
    redefines: Option<String>,
}

/// One tree node of the level hierarchy: an item with its children.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    /// The item this node declares.
    item: Item,
    /// Deeper-level items nested under it.
    children: Vec<Node>,
}

/// One name bound so far: its span and whether it decodes numeric.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NameBinding {
    /// The field or group name, folded to uppercase.
    name: String,
    /// Byte offset in the record.
    offset: usize,
    /// Bytes the binding holds.
    len: usize,
    /// Whether the binding decodes to a number.
    numeric: bool,
}

/// One name resolution against the bindings laid out so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// Exactly one earlier binding, with its offset and length.
    One(usize, usize),
    /// No earlier binding carries the name.
    Missing,
    /// More than one earlier binding carries the name.
    Ambiguous,
}

/// Resolves a name against the bindings laid out so far, refusing to guess
/// between two.
fn resolve_name(bindings: &[NameBinding], target: &str) -> Resolution {
    let mut found: Option<(usize, usize)> = None;
    let mut count: usize = 0;
    for entry in bindings.iter() {
        if entry.name.as_str() == target {
            count = count.saturating_add(1);
            found = Some((entry.offset, entry.len));
        }
    }
    match found {
        Some((offset, len)) if count == 1 => Resolution::One(offset, len),
        Some(_) => Resolution::Ambiguous,
        None => Resolution::Missing,
    }
}

/// Whether a binding name marks a numeric holder for `DEPENDING ON`.
fn binding_is_numeric(bindings: &[NameBinding], target: &str) -> bool {
    bindings
        .iter()
        .any(|entry| entry.name.as_str() == target && entry.numeric)
}

/// How one slot's bytes are stored: text, or a number with its digit count
/// and whether it carries a sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotKind {
    /// `PIC X` display text.
    Text,
    /// Zoned display digits.
    Zoned {
        /// Digits the picture holds.
        digits: usize,
        /// Whether an `S` signed the picture.
        signed: bool,
    },
    /// Packed `COMP-3` nibbles.
    Packed {
        /// Digits the picture holds.
        digits: usize,
        /// Whether an `S` signed the picture.
        signed: bool,
    },
    /// Big-endian binary `COMP`/`COMP-4`/`BINARY`.
    Binary {
        /// Digits the picture holds.
        digits: usize,
        /// Whether an `S` signed the picture.
        signed: bool,
    },
}

/// One laid-out value slot: a leaf whose repetition the layout expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot {
    /// The field name, folded to uppercase.
    name: String,
    /// Byte offset of the first occurrence in the record.
    offset: usize,
    /// Bytes one occurrence holds.
    stride: usize,
    /// Bytes all occurrences hold at most.
    len: usize,
    /// The storage shape.
    kind: SlotKind,
    /// Implied decimal places.
    scale: u32,
    /// Repetition floor: 1, or the `DEPENDING ON` floor.
    min_occurs: usize,
    /// Repetition ceiling: 1, the fixed count, or the `DEPENDING ON` ceiling.
    max_occurs: usize,
    /// The count field for `DEPENDING ON`; `None` for a fixed layout.
    depending_on: Option<String>,
}

/// A parsed copybook: laid-out fields over fixed-width records.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordLayout {
    /// Laid-out fields in declaration order.
    fields: Vec<LayoutField>,
    /// Bytes one record holds.
    record_len: usize,
    /// The 88 condition names the copybook keeps.
    conditions: Vec<String>,
    /// The EBCDIC page display text is written in.
    page: CodePage,
}

impl RecordLayout {
    /// Assembles a layout from laid-out fields and the record length.
    pub(crate) fn from_parts(fields: Vec<LayoutField>, record_len: usize) -> Self {
        Self {
            fields,
            record_len,
            conditions: Vec::new(),
            page: CodePage::Cp037,
        }
    }

    /// The same layout reading and writing display text on `page`.
    ///
    /// A record is EBCDIC throughout: zoned digits carry the `F` zone and
    /// `PIC X` text is on one code page, 037 unless this names another.
    #[must_use]
    pub const fn with_code_page(mut self, page: CodePage) -> Self {
        self.page = page;
        self
    }

    /// The EBCDIC page display text is read and written on.
    #[must_use]
    pub const fn code_page(&self) -> CodePage {
        self.page
    }

    /// Parses copybook text into a record layout.
    ///
    /// Free format, one statement per line, each ending in a period; names
    /// fold to uppercase. Only the documented clauses are admitted.
    ///
    /// # Errors
    ///
    /// [`CopybookError`] with the one-based line of whatever the text holds
    /// outside the admitted grammar, or [`CopybookError::Limit`] past a ceiling.
    ///
    /// [`CopybookError`]: model::CopybookError
    pub fn parse(text: &str, limits: &Limits) -> Result<Self, CopybookError> {
        limits.check_bytes(text.len())?;
        let mut items: Vec<Item> = Vec::new();
        let mut conditions: Vec<String> = Vec::new();
        for (index, raw_line) in text.lines().enumerate() {
            let line_no = index.saturating_add(1);
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('*') {
                continue;
            }
            let Some(dotted) = trimmed.strip_suffix('.') else {
                let refusal = Err(CopybookError::UnknownClause {
                    line: line_no,
                    text: String::from(trimmed),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a line with no closing period");
                return refusal;
            };
            let upper = dotted.to_uppercase();
            let mut tokens = upper.split_whitespace();
            let Some(level_text) = tokens.next() else {
                let refusal = Err(CopybookError::UnknownClause {
                    line: line_no,
                    text: String::from(trimmed),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing an empty statement");
                return refusal;
            };
            let Ok(level) = level_text.parse::<u8>() else {
                let refusal = Err(CopybookError::BadLevel {
                    line: line_no,
                    text: String::from(level_text),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a non-numeric level");
                return refusal;
            };
            let Some(name) = tokens.next().map(String::from) else {
                let refusal = Err(CopybookError::UnknownClause {
                    line: line_no,
                    text: String::from(trimmed),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a level with no name");
                return refusal;
            };
            if level == 66 {
                let refusal = Err(CopybookError::RenamesRefused {
                    line: line_no,
                    name,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a RENAMES entry");
                return refusal;
            }
            if level == 88 {
                conditions.push(name);
                continue;
            }
            if !(1..=49).contains(&level) {
                let refusal = Err(CopybookError::BadLevel {
                    line: line_no,
                    text: String::from(level_text),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a level past 01-49");
                return refusal;
            }
            let rest: Vec<&str> = tokens.collect();
            let item = grammar::parse_item(line_no, level, name, &rest)?;
            items.push(item);
        }
        let tree = layout::build_tree(items, limits)?;
        let mut layout = layout::expand_tree(tree, limits)?;
        layout.conditions = conditions;
        Ok(layout)
    }

    /// The laid-out fields in declaration order.
    #[must_use]
    pub fn fields(&self) -> &[LayoutField] {
        &self.fields
    }

    /// Bytes one record holds.
    #[must_use]
    pub const fn record_len(&self) -> usize {
        self.record_len
    }

    /// The 88 condition names the copybook keeps.
    #[must_use]
    pub fn condition_names(&self) -> &[String] {
        &self.conditions
    }

    /// Decodes one fixed-width record into values aligned with [`fields`].
    ///
    /// [`fields`]: Self::fields
    ///
    /// # Errors
    ///
    /// [`CopybookError`] naming the field whose slice, count or digits fail,
    /// or [`CopybookError::Limit`] past a ceiling.
    ///
    /// [`CopybookError`]: model::CopybookError
    pub fn decode(&self, bytes: &[u8], limits: &Limits) -> Result<Vec<FieldValue>, CopybookError> {
        limits.check_bytes(bytes.len())?;
        if bytes.len() != self.record_len {
            let refusal = Err(CopybookError::LengthMismatch {
                need: self.record_len,
                have: bytes.len(),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a record past its layout length");
            return refusal;
        }
        let mut values: Vec<FieldValue> = Vec::with_capacity(self.fields.len());
        let mut numbers: Vec<(String, i128)> = Vec::new();
        for field in self.fields.iter() {
            let value = reader::decode_field(field, bytes, &numbers, limits, self.page)?;
            if let FieldValue::Number(ref drawn) = value {
                numbers.push((String::from(field.name()), drawn.coefficient()));
            }
            values.push(value);
        }
        Ok(values)
    }

    /// Encodes values aligned with [`fields`] back into one record.
    ///
    /// [`fields`]: Self::fields
    ///
    /// # Errors
    ///
    /// [`CopybookError`] naming the field whose value, count or digits fail,
    /// or [`CopybookError::Limit`] past a ceiling.
    ///
    /// [`CopybookError`]: model::CopybookError
    pub fn encode(&self, values: &[FieldValue], limits: &Limits) -> Result<Vec<u8>, CopybookError> {
        limits.check_bytes(self.record_len)?;
        if values.len() != self.fields.len() {
            let refusal = Err(CopybookError::LengthMismatch {
                need: self.fields.len(),
                have: values.len(),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing values past the field count");
            return refusal;
        }
        let mut out = vec![EBCDIC_SPACE; self.record_len];
        let mut numbers: Vec<(String, i128)> = Vec::new();
        for (field, value) in self.fields.iter().zip(values.iter()) {
            writer::encode_field(field, value, &mut out, &numbers, limits, self.page)?;
            if let FieldValue::Number(ref drawn) = *value {
                numbers.push((String::from(field.name()), drawn.coefficient()));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::RecordLayout;
    use super::model::{CopybookError, FieldValue};
    use crate::legacy::decimal::{Decimal, encode_packed};
    use crate::legacy::ebcdic::CodePage;
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(4096, 256, 16, 8, 16384);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), CopybookError>;

    /// The small customer copybook the unit tests parse.
    const CUSTOMER: &str = "01 CUSTOMER.\n05 CUST-ID PIC 9(6).\n05 CUST-NAME PIC X(10).\n05 CUST-BAL PIC S9(5)V99 COMP-3.\n05 CUST-BORN-DATE PIC 9(8).\n";

    #[test]
    fn a_small_record_decodes_and_encodes() -> UnitResult {
        let layout = RecordLayout::parse(CUSTOMER, &BOUNDS)?;
        assert_eq!(
            layout.record_len(),
            6_usize
                .saturating_add(10)
                .saturating_add(4)
                .saturating_add(8),
            "the layout must sum its fields"
        );
        let balance = encode_packed(Decimal::new(12_345, 2), 7, true, &BOUNDS)?;
        assert_eq!(
            balance,
            vec![0x00, 0x12, 0x34, 0x5c],
            "123.45 packed in seven digits must be 00 12 34 5C"
        );
        Ok(())
    }

    /// A mainframe record as it arrives, EBCDIC throughout: zoned id
    /// `000042`, 037 text `ADA` space-filled, packed 123.45, zoned date.
    #[test]
    fn a_text_date_and_number_round_trip() -> UnitResult {
        let layout = RecordLayout::parse(CUSTOMER, &BOUNDS)?;
        let mut record: Vec<u8> = vec![0xf0, 0xf0, 0xf0, 0xf0, 0xf4, 0xf2];
        record.extend_from_slice(&[0xc1, 0xc4, 0xc1, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40]);
        record.extend_from_slice(&encode_packed(Decimal::new(12_345, 2), 7, true, &BOUNDS)?);
        record.extend_from_slice(&[0xf1, 0xf9, 0xf9, 0xf0, 0xf0, 0xf1, 0xf3, 0xf1]);
        let values = layout.decode(&record, &BOUNDS)?;
        assert_eq!(
            values.len(),
            4,
            "four declared fields must decode to four values"
        );
        assert_eq!(
            values.get(..3),
            Some(
                [
                    FieldValue::Number(Decimal::new(42, 0)),
                    FieldValue::Text(String::from("ADA       ")),
                    FieldValue::Number(Decimal::new(12_345, 2)),
                ]
                .as_slice()
            ),
            "the id, the 037 name and the balance must decode"
        );
        assert_eq!(
            layout.encode(&values, &BOUNDS)?,
            record,
            "decode then encode must return the same bytes"
        );
        let international = RecordLayout::parse(CUSTOMER, &BOUNDS)?.with_code_page(CodePage::Cp500);
        let mut renamed = values;
        if let Some(name) = renamed.get_mut(1) {
            *name = FieldValue::Text(String::from("[ADA]!    "));
        }
        let coded = international.encode(&renamed, &BOUNDS)?;
        assert_eq!(
            coded.get(6..11),
            Some([0x4a, 0xc1, 0xc4, 0xc1, 0x5a].as_slice()),
            "500 text must spell its brackets on the national bytes"
        );
        assert_eq!(
            international.decode(&coded, &BOUNDS)?,
            renamed,
            "500 text must round-trip"
        );
        Ok(())
    }

    #[test]
    fn renames_is_refused_with_its_name() {
        let text = "01 R.\n66 RENAMED RENAMES FOO.\n";
        assert!(
            matches!(
                RecordLayout::parse(text, &BOUNDS),
                Err(CopybookError::RenamesRefused { .. })
            ),
            "a 66 RENAMES entry must be refused"
        );
    }

    #[test]
    fn an_unknown_clause_names_its_line() {
        let text = "01 R.\n05 FOO PIC X(4) SYNC.\n";
        assert!(
            matches!(
                RecordLayout::parse(text, &BOUNDS),
                Err(CopybookError::UnknownClause { line: 2, .. })
            ),
            "an unknown clause must be located at line two"
        );
    }

    #[test]
    fn condition_names_are_kept() -> UnitResult {
        let text = "01 R.\n05 FLAG PIC X.\n88 FLAG-OK VALUE 'Y'.\n";
        let layout = RecordLayout::parse(text, &BOUNDS)?;
        let names = layout.condition_names();
        assert_eq!(names.len(), 1, "one 88 entry must be kept as one name");
        assert_eq!(
            names.first(),
            Some(&String::from("FLAG-OK")),
            "the 88 name must be kept"
        );
        Ok(())
    }
}
