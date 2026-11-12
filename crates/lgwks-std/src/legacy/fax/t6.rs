//! T.6 two-dimensional coding: G4 strips (ITU-T T.6 §2.2).
//!
//! Every row is coded against the row above (the row before the strip is
//! white) in pass, vertical and horizontal modes. [`decode_2d_row`] is shared
//! with the G3 2D rows of `t4`; [`encode_g4`] chooses modes exactly as T.6
//! Figure 3 does and ends the strip with the EOFB of two EOLs.
//!
//! A mode resolves in one index of the mode lookup. Within a row a0 never
//! moves left, so the changing elements at or before it never matter again:
//! b1, b2 and a1 are found from a cursor that only moves forward, where a
//! search from the row's start made every row quadratic in its changes.

use super::super::limits::Limits;
use super::t4::{BitReader, BitWriter, decode_run, emit_run};
use super::tables::{
    EOL_BITS, EOL_LEN, MODE_HORIZ, MODE_LOOKUP_BITS, MODE_NUMBER_HORIZ, MODE_NUMBER_PASS,
    MODE_PASS, MODE_VERTICAL, MODE_VERTICAL_BASE, mode_lookup,
};
use super::{Strip, TiffError, row_changes};

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

/// Reads one mode code. The mode codes are prefix-free, so the slot the
/// next seven bits index names the one mode they start with; no mode there,
/// or a mode longer than the bits the strip still holds, is corrupt.
fn read_mode(reader: &mut BitReader<'_>, strip: &Strip<'_>, row: usize) -> Result<Mode, TiffError> {
    let (bits, held) = reader.peek(MODE_LOOKUP_BITS);
    let slot = usize::try_from(bits)
        .ok()
        .and_then(|index| mode_lookup().get(index).copied());
    let Some(slot) = slot else {
        return crate::legacy::refuse(strip.corrupt(row));
    };
    let len = slot.len();
    if len == 0 || usize::from(len) > held {
        return crate::legacy::refuse(strip.corrupt(row));
    }
    let mode = match slot.value() {
        MODE_NUMBER_PASS => Mode::Pass,
        MODE_NUMBER_HORIZ => Mode::Horizontal,
        number => match VERTICAL_OFFSETS.get(number.saturating_sub(MODE_VERTICAL_BASE)) {
            Some(&offset) if number >= MODE_VERTICAL_BASE => Mode::Vertical(offset),
            _ => return crate::legacy::refuse(strip.corrupt(row)),
        },
    };
    reader.skip(len);
    Ok(mode)
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

/// Moves `cursor` past every element at or before a0. The elements ascend,
/// so the ones before the cursor never lie past a later a0 either.
fn advance(elements: &[usize], cursor: &mut usize, a0: Option<usize>) {
    while elements
        .get(*cursor)
        .is_some_and(|&element| !past(element, a0))
    {
        *cursor = cursor.saturating_add(1);
    }
}

/// b1 and b2 (ITU-T T.6 §2.2.2): b1 is the first changing element on the
/// reference row past a0 of the colour opposite a0's, b2 the next one; an
/// absent element sits at the row's end. Even indices turn black, so b1 is
/// the first index from the cursor whose parity matches a0's colour.
fn reference_pair(
    reference: &[usize],
    cursor: &mut usize,
    a0: Option<usize>,
    white: bool,
    width: usize,
) -> (usize, usize) {
    advance(reference, cursor, a0);
    let index = if cursor.is_multiple_of(2) == white {
        *cursor
    } else {
        cursor.saturating_add(1)
    };
    match reference.get(index) {
        Some(&b1) => match reference.get(index.saturating_add(1)) {
            Some(&b2) => (b1, b2),
            None => (b1, width),
        },
        None => (width, width),
    }
}

/// The first changing element of the coding row past a0, or the row's end.
fn next_change(changes: &[usize], cursor: &mut usize, a0: Option<usize>, width: usize) -> usize {
    advance(changes, cursor, a0);
    match changes.get(*cursor) {
        Some(&element) => element,
        None => width,
    }
}

/// Decodes one 2D row against `reference`, the row above's changing
/// elements, recording the row's own into `changes`.
pub(crate) fn decode_2d_row(
    reader: &mut BitReader<'_>,
    strip: &mut Strip<'_>,
    row: usize,
    reference: &[usize],
    changes: &mut RowChanges<'_>,
) -> Result<(), TiffError> {
    let width = strip.width();
    let mut a0: Option<usize> = None;
    let mut white = true;
    let mut cursor: usize = 0;
    while origin(a0) < width {
        let start = origin(a0);
        let (b1, b2) = reference_pair(reference, &mut cursor, a0, white, width);
        match read_mode(reader, strip, row)? {
            Mode::Pass => {
                changes.paint(strip, row, (start, b2), white)?;
                a0 = Some(b2);
            }
            Mode::Horizontal => {
                let a1 = start.saturating_add(decode_run(reader, white, strip, row)?);
                changes.paint(strip, row, (start, a1), white)?;
                let a2 = a1.saturating_add(decode_run(reader, !white, strip, row)?);
                changes.paint(strip, row, (a1, a2), !white)?;
                a0 = Some(a2);
            }
            Mode::Vertical(offset) => {
                let a1 = match b1.checked_add_signed(offset) {
                    Some(a1) if past(a1, a0) => a1,
                    _ => return crate::legacy::refuse(strip.corrupt(row)),
                };
                changes.paint(strip, row, (start, a1), white)?;
                a0 = Some(a1);
                white = !white;
            }
        }
    }
    Ok(())
}

/// The row above's changing elements and the row being decoded's, swapped
/// row after row, so a strip of any height holds two buffers, each bounded
/// by the row width; before the first row the reference is the white row.
pub(crate) struct Reference {
    /// The changing elements of the row above.
    above: Vec<usize>,
    /// The changing elements recorded for the row being decoded.
    current: Vec<usize>,
}

impl Reference {
    /// The white row before a strip.
    pub(crate) const fn new() -> Self {
        Self {
            above: Vec::new(),
            current: Vec::new(),
        }
    }

    /// The row above's changes, and a recorder for the next row's.
    pub(crate) fn begin_row(&mut self) -> (&[usize], RowChanges<'_>) {
        self.current.clear();
        (
            &self.above,
            RowChanges {
                out: &mut self.current,
                black: false,
            },
        )
    }

    /// Makes the row just decoded the reference of the row after it.
    pub(crate) const fn roll(&mut self) {
        core::mem::swap(&mut self.above, &mut self.current);
    }
}

/// Paints one row's runs and records its changing elements as it goes.
///
/// The page bitmap starts zeroed and a decoded row's runs cover it left to
/// right without a gap, so a changing element is exactly where a non-empty
/// run's colour differs from the run before it, the pixel before the row
/// counting as white. That is the reference the next row needs, read from
/// the decode instead of scanning the bitmap back.
pub(crate) struct RowChanges<'buffer> {
    /// The changing elements so far, ascending, each below the width.
    out: &'buffer mut Vec<usize>,
    /// The colour of the last non-empty run.
    black: bool,
}

impl RowChanges<'_> {
    /// Paints `from..to` of `row` in one colour and records the change it
    /// makes, if any.
    pub(crate) fn paint(
        &mut self,
        strip: &mut Strip<'_>,
        row: usize,
        span: (usize, usize),
        white: bool,
    ) -> Result<(), TiffError> {
        let (from, to) = span;
        strip.paint(row, from, to, white)?;
        if from < to && white == self.black {
            self.out.push(from);
            self.black = !white;
        }
        Ok(())
    }
}

/// Decodes one G4 strip: every row in 2D, the first against a white row.
pub(crate) fn decode_g4_strip(
    data: &[u8],
    strip: &mut Strip<'_>,
    limits: &Limits,
) -> Result<(), TiffError> {
    let mut reader = BitReader::new(data);
    let mut reference = Reference::new();
    for row in strip.rows() {
        limits.check_records(row.saturating_add(1))?;
        let (above, mut changes) = reference.begin_row();
        decode_2d_row(&mut reader, strip, row, above, &mut changes)?;
        reference.roll();
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
    let (mut coding_cursor, mut reference_cursor) = (0_usize, 0_usize);
    while origin(a0) < width {
        let start = origin(a0);
        let a1 = next_change(coding, &mut coding_cursor, a0, width);
        let (b1, b2) = reference_pair(reference, &mut reference_cursor, a0, white, width);
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
        let mut lookahead = coding_cursor;
        let a2 = next_change(coding, &mut lookahead, Some(a1), width);
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

#[cfg(test)]
mod tests {
    use super::super::{Strip, TiffError};
    use super::decode_g4_strip;
    use crate::legacy::limits::Limits;

    /// Decodes `stream` as one G4 row `width` pixels wide.
    fn one_row(stream: &[u8], width: usize) -> Result<Vec<u8>, TiffError> {
        let row_bytes = width.div_ceil(8);
        let mut bitmap = vec![0_u8; row_bytes];
        let mut strip = Strip::new(&mut bitmap, (row_bytes, width), 0..1, 0);
        decode_g4_strip(stream, &mut strip, &Limits::new(4096, 64, 64, 8, 4096))?;
        Ok(bitmap)
    }

    /// The reader peeks past the strip as zeros, which must never complete
    /// a code: `001 0111 1` is H, W2 and the first bit of a black run, and
    /// zeros after it would spell `10`, black run 3, filling a five-pixel
    /// row. The strip ends there, so the row is corrupt; with its second
    /// bit present the same row decodes.
    #[test]
    fn a_code_cut_by_the_strip_end_is_refused_not_padded() {
        let corrupt = Err(TiffError::CorruptStrip { page: 0, row: 0 });
        assert_eq!(
            one_row(&[0b0010_1111], 5),
            corrupt,
            "a black run whose second bit is past the strip must be refused"
        );
        assert_eq!(
            one_row(&[0b0010_1111, 0b0000_0000], 5),
            Ok(vec![0b0011_1000]),
            "with its second bit present the row is W2 B3"
        );
        assert_eq!(
            one_row(&[0b0000_0001], 8),
            corrupt,
            "seven zeros then a one start no mode"
        );
        assert_eq!(
            one_row(&[0b0100_0000], 8),
            corrupt,
            "a lone 01 must not read as VL1 through the zeros past the strip"
        );
    }
}
