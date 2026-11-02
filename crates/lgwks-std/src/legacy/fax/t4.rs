//! T.4 run coding: G3 strips (ITU-T T.4 §4.1, §4.2).
//!
//! [`decode_g3_strip`] turns strip bytes into bitmap rows: one-dimensional
//! rows, or under TIFF `T4Options` bit 0 a tag bit after every EOL choosing a
//! 1D row or a 2D row coded against the row above. EOLs may carry fill bits.
//! [`encode_g3`] writes one continuous 1D stream: an EOL before every row,
//! then the return-to-control of six EOLs.

use super::super::limits::Limits;
use super::t6;
use super::tables::{EOL_BITS, EOL_LEN, EXTENDED_MAKEUP, MAX_CODE_LEN, MAX_MAKEUP, colour_tables};
use super::{BitReader, BitWriter, Strip, TiffError, row_changes};

/// The run a `(bits, length)` code stands for in one table.
fn lookup(table: &[(u16, u8, u16)], len: u8, bits: u16) -> Option<usize> {
    table
        .iter()
        .find(|entry| entry.1 == len && entry.0 == bits)
        .map(|entry| usize::from(entry.2))
}

/// The `(bits, length)` code for an exact run in one table.
fn code_for(table: &[(u16, u8, u16)], run: usize) -> Option<(u16, u8)> {
    table
        .iter()
        .find(|entry| usize::from(entry.2) == run)
        .map(|entry| (entry.0, entry.1))
}

/// Reads one run of one colour: make-up codes, then a terminating code.
///
/// Each colour's code set is prefix-free, so the first exact hit at any
/// length is the code; a prefix past the longest code is corrupt, and so is
/// a run past the row.
pub(crate) fn decode_run(
    reader: &mut BitReader<'_>,
    white: bool,
    strip: &Strip<'_>,
    row: usize,
) -> Result<usize, TiffError> {
    let (terms, makeup) = colour_tables(white);
    let mut run: usize = 0;
    loop {
        let mut len: u8 = 0;
        let mut bits: u16 = 0;
        let extra = loop {
            let Some(bit) = reader.next_bit() else {
                return crate::legacy::refuse(strip.corrupt(row));
            };
            bits = bits.saturating_mul(2).saturating_add(u16::from(bit));
            len = len.saturating_add(1);
            if let Some(term) = lookup(terms, len, bits) {
                return Ok(run.saturating_add(term));
            }
            if let Some(extra) =
                lookup(makeup, len, bits).or_else(|| lookup(&EXTENDED_MAKEUP, len, bits))
            {
                break extra;
            }
            if len >= MAX_CODE_LEN {
                return crate::legacy::refuse(strip.corrupt(row));
            }
        };
        run = run.saturating_add(extra);
        if run > strip.width() {
            return crate::legacy::refuse(strip.corrupt(row));
        }
    }
}

/// Skips EOL codes and the fill zeros before each, returning how many were
/// skipped. A one after fewer than eleven zeros is row data, put back.
pub(crate) fn skip_eols(reader: &mut BitReader<'_>) -> usize {
    let mut skipped: usize = 0;
    loop {
        let mut zeros: usize = 0;
        loop {
            match reader.next_bit() {
                Some(false) => zeros = zeros.saturating_add(1),
                Some(true) => break,
                None => {
                    if zeros < usize::from(EOL_LEN) {
                        reader.unread(&vec![false; zeros]);
                    }
                    return skipped;
                }
            }
        }
        if zeros >= usize::from(EOL_LEN.saturating_sub(1)) {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let mut put_back = vec![false; zeros];
        put_back.push(true);
        reader.unread(&put_back);
        return skipped;
    }
}

/// Decodes one 1D row: runs alternating from white until the row is full.
fn decode_1d_row(
    reader: &mut BitReader<'_>,
    strip: &mut Strip<'_>,
    row: usize,
) -> Result<(), TiffError> {
    let mut x: usize = 0;
    let mut white = true;
    while x < strip.width() {
        let run = decode_run(reader, white, strip, row)?;
        let end = x.saturating_add(run);
        strip.paint(row, x, end, white)?;
        x = end;
        white = !white;
    }
    Ok(())
}

/// Decodes one G3 strip. With `two_d` (TIFF `T4Options` bit 0) each EOL is
/// followed by a tag bit: one for a 1D row, zero for a 2D row coded against
/// the row above, the row before the strip counting as white.
pub(crate) fn decode_g3_strip(
    data: &[u8],
    strip: &mut Strip<'_>,
    two_d: bool,
    limits: &Limits,
) -> Result<(), TiffError> {
    let mut reader = BitReader::new(data);
    let mut reference: Vec<usize> = Vec::new();
    for row in strip.rows() {
        limits.check_records(row.saturating_add(1))?;
        skip_eols(&mut reader);
        let coded_2d = if two_d {
            let Some(tag) = reader.next_bit() else {
                return crate::legacy::refuse(strip.corrupt(row));
            };
            !tag
        } else {
            false
        };
        if coded_2d {
            t6::decode_2d_row(&mut reader, strip, row, &reference)?;
        } else {
            decode_1d_row(&mut reader, strip, row)?;
        }
        if two_d {
            reference = strip.changes(row);
        }
    }
    Ok(())
}

/// Encodes rows as one continuous T.4 1D stream: EOL and runs per row, then
/// the six-EOL return-to-control.
pub(crate) fn encode_g3(rows: &[&[u8]], width: usize, page: usize) -> Result<Vec<u8>, TiffError> {
    let mut writer = BitWriter::new();
    for row in rows.iter() {
        writer.push_code((EOL_BITS, EOL_LEN));
        let changes = row_changes(row, width);
        let mut x: usize = 0;
        let mut white = true;
        for &change in changes.iter().chain(core::iter::once(&width)) {
            emit_run(&mut writer, change.saturating_sub(x), white, page)?;
            x = change;
            white = !white;
        }
    }
    for _ in 0..6 {
        writer.push_code((EOL_BITS, EOL_LEN));
    }
    Ok(writer.finish())
}

/// Emits one run: repeated 2560 make-ups, one make-up for the remaining
/// multiple of 64, then the terminating code (ITU-T T.4 §4.1.1).
pub(crate) fn emit_run(
    writer: &mut BitWriter,
    run: usize,
    white: bool,
    page: usize,
) -> Result<(), TiffError> {
    let (terms, makeup) = colour_tables(white);
    let missing = || TiffError::Unsupported {
        page,
        what: format!("a run of {run} past the T.4 tables"),
    };
    let mut rest = run;
    while rest >= MAX_MAKEUP.saturating_add(64) {
        writer.push_code(code_for(&EXTENDED_MAKEUP, MAX_MAKEUP).ok_or_else(missing)?);
        rest = rest.saturating_sub(MAX_MAKEUP);
    }
    let tail = rest.rem_euclid(64);
    let bulk = rest.saturating_sub(tail);
    if bulk > 0 {
        let code = code_for(makeup, bulk).or_else(|| code_for(&EXTENDED_MAKEUP, bulk));
        writer.push_code(code.ok_or_else(missing)?);
    }
    writer.push_code(code_for(terms, tail).ok_or_else(missing)?);
    Ok(())
}
