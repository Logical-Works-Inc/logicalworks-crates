//! T.6 two-dimensional coding: G4 strips (ITU-T T.6 §2.2).
//!
//! Every row is coded against the row above (the row before the strip is
//! white) in pass, vertical and horizontal modes. [`decode_2d_row`] is shared
//! with the G3 2D rows of `t4`; [`encode_g4`] chooses modes exactly as T.6
//! Figure 3 does and ends the strip with the EOFB of two EOLs.

use super::super::limits::Limits;
use super::t4::{decode_run, emit_run};
use super::tables::{EOL_BITS, EOL_LEN, MODE_HORIZ, MODE_PASS, MODE_VERTICAL};
use super::{BitReader, BitWriter, Strip, TiffError, row_changes};

/// The vertical offsets `MODE_VERTICAL` codes, in its order.
const VERTICAL_OFFSETS: [isize; 7] = [-3, -2, -1, 0, 1, 2, 3];

/// One 2D coding mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// a0 moves under b2; the colour holds.
    Pass,
    /// Two runs follow: a0 to a1, then a1 to a2.
    Horizontal,
    /// a1 sits this offset from b1; the colour flips.
    Vertical(isize),
}

/// Reads one mode code. The mode codes are prefix-free, so the first exact
/// hit is the mode; seven bits with no hit is corrupt.
fn read_mode(reader: &mut BitReader<'_>, strip: &Strip<'_>, row: usize) -> Result<Mode, TiffError> {
    let mut code: (u16, u8) = (0, 0);
    while code.1 < 7 {
        let Some(bit) = reader.next_bit() else {
            return crate::legacy::refuse(strip.corrupt(row));
        };
        code = (
            code.0.saturating_mul(2).saturating_add(u16::from(bit)),
            code.1.saturating_add(1),
        );
        if code == MODE_PASS {
            return Ok(Mode::Pass);
        }
        if code == MODE_HORIZ {
            return Ok(Mode::Horizontal);
        }
        let vertical = MODE_VERTICAL
            .iter()
            .zip(VERTICAL_OFFSETS.iter())
            .find(|pair| *pair.0 == code);
        if let Some((_, &offset)) = vertical {
            return Ok(Mode::Vertical(offset));
        }
    }
    Err(strip.corrupt(row))
}

/// The coding position a0 as a pixel: the imaginary start counts as zero.
const fn origin(a0: Option<usize>) -> usize {
    match a0 {
        Some(at) => at,
        None => 0,
    }
}

/// Whether a changing element lies past a0; every element lies past the
/// imaginary start before the row.
fn past(element: usize, a0: Option<usize>) -> bool {
    match a0 {
        Some(at) => element > at,
        None => true,
    }
}

/// b1 and b2 (ITU-T T.6 §2.2.2): b1 is the first changing element on the
/// reference row past a0 of the colour opposite a0's, b2 the next one; an
/// absent element sits at the row's end.
fn reference_pair(
    reference: &[usize],
    a0: Option<usize>,
    white: bool,
    width: usize,
) -> (usize, usize) {
    let found = reference
        .iter()
        .enumerate()
        .find(|&(index, &element)| past(element, a0) && index.is_multiple_of(2) == white);
    match found {
        Some((index, &b1)) => match reference.get(index.saturating_add(1)) {
            Some(&b2) => (b1, b2),
            None => (b1, width),
        },
        None => (width, width),
    }
}

/// The first changing element of the coding row past a0, or the row's end.
fn next_change(changes: &[usize], a0: Option<usize>, width: usize) -> usize {
    match changes.iter().find(|&&element| past(element, a0)) {
        Some(&element) => element,
        None => width,
    }
}

/// Decodes one 2D row against `reference`, the row above's changing
/// elements.
pub(crate) fn decode_2d_row(
    reader: &mut BitReader<'_>,
    strip: &mut Strip<'_>,
    row: usize,
    reference: &[usize],
) -> Result<(), TiffError> {
    let width = strip.width();
    let mut a0: Option<usize> = None;
    let mut white = true;
    while origin(a0) < width {
        let start = origin(a0);
        let (b1, b2) = reference_pair(reference, a0, white, width);
        match read_mode(reader, strip, row)? {
            Mode::Pass => {
                strip.paint(row, start, b2, white)?;
                a0 = Some(b2);
            }
            Mode::Horizontal => {
                let a1 = start.saturating_add(decode_run(reader, white, strip, row)?);
                strip.paint(row, start, a1, white)?;
                let a2 = a1.saturating_add(decode_run(reader, !white, strip, row)?);
                strip.paint(row, a1, a2, !white)?;
                a0 = Some(a2);
            }
            Mode::Vertical(offset) => {
                let a1 = match b1.checked_add_signed(offset) {
                    Some(a1) if past(a1, a0) => a1,
                    _ => return crate::legacy::refuse(strip.corrupt(row)),
                };
                strip.paint(row, start, a1, white)?;
                a0 = Some(a1);
                white = !white;
            }
        }
    }
    Ok(())
}

/// Decodes one G4 strip: every row in 2D, the first against a white row.
pub(crate) fn decode_g4_strip(
    data: &[u8],
    strip: &mut Strip<'_>,
    limits: &Limits,
) -> Result<(), TiffError> {
    let mut reader = BitReader::new(data);
    let mut reference: Vec<usize> = Vec::new();
    for row in strip.rows() {
        limits.check_records(row.saturating_add(1))?;
        decode_2d_row(&mut reader, strip, row, &reference)?;
        reference = strip.changes(row);
    }
    Ok(())
}

/// Encodes one row in 2D against `reference`, choosing pass, vertical or
/// horizontal mode at each a0 as ITU-T T.6 Figure 3 does.
fn encode_2d_row(
    writer: &mut BitWriter,
    coding: &[usize],
    reference: &[usize],
    width: usize,
    page: usize,
) -> Result<(), TiffError> {
    let mut a0: Option<usize> = None;
    let mut white = true;
    while origin(a0) < width {
        let start = origin(a0);
        let a1 = next_change(coding, a0, width);
        let (b1, b2) = reference_pair(reference, a0, white, width);
        if b2 < a1 {
            writer.push_code(MODE_PASS);
            a0 = Some(b2);
            continue;
        }
        let vertical = MODE_VERTICAL
            .iter()
            .zip(VERTICAL_OFFSETS.iter())
            .find(|pair| b1.checked_add_signed(*pair.1) == Some(a1));
        if let Some((&code, _)) = vertical {
            writer.push_code(code);
            a0 = Some(a1);
            white = !white;
            continue;
        }
        let a2 = next_change(coding, Some(a1), width);
        writer.push_code(MODE_HORIZ);
        emit_run(writer, a1.saturating_sub(start), white, page)?;
        emit_run(writer, a2.saturating_sub(a1), !white, page)?;
        a0 = Some(a2);
    }
    Ok(())
}

/// Encodes rows as one G4 stream: every row in 2D against the row above,
/// then the EOFB of two EOLs.
pub(crate) fn encode_g4(rows: &[&[u8]], width: usize, page: usize) -> Result<Vec<u8>, TiffError> {
    let mut writer = BitWriter::new();
    let mut reference: Vec<usize> = Vec::new();
    for row in rows.iter() {
        let coding = row_changes(row, width);
        encode_2d_row(&mut writer, &coding, &reference, width, page)?;
        reference = coding;
    }
    writer.push_code((EOL_BITS, EOL_LEN));
    writer.push_code((EOL_BITS, EOL_LEN));
    Ok(writer.finish())
}
