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
//!
//! a0 is carried as the first pixel a changing element may sit on, `floor`:
//! zero at the imaginary start before the row, a0 plus one after it, so
//! "past a0" is one comparison with no `Option` to match per element.

use super::super::limits::Limits;
use super::t4::{BitReader, BitWriter, decode_run, emit_run};
use super::tables::{
    EOL_BITS, EOL_LEN, Lookups, MODE_HORIZ, MODE_LOOKUP_BITS, MODE_NUMBER_HORIZ, MODE_NUMBER_PASS,
    MODE_PASS, MODE_VERTICAL, MODE_VERTICAL_BASE, Slot,
};
use super::{Corrupt, RowAt, Strip, TiffError, cut, row_changes, set_span};

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

/// Reads one mode code through the mode lookup; a slot naming no mode is
/// corrupt.
#[inline]
fn read_mode(reader: &mut BitReader<'_>, table: &[Slot]) -> Result<Mode, Corrupt> {
    let slot = reader.read_slot(table, MODE_LOOKUP_BITS)?;
    Ok(match slot.value() {
        MODE_NUMBER_PASS => Mode::Pass,
        MODE_NUMBER_HORIZ => Mode::Horizontal,
        number => match VERTICAL_OFFSETS.get(number.saturating_sub(MODE_VERTICAL_BASE)) {
            Some(&offset) if number >= MODE_VERTICAL_BASE => Mode::Vertical(offset),
            _ => return cut(),
        },
    })
}

/// Moves `cursor` past every element below `floor`. The elements ascend,
/// so the ones before the cursor never reach a later floor either.
#[inline]
fn advance(elements: &[usize], cursor: &mut usize, floor: usize) {
    while elements
        .get(*cursor)
        .is_some_and(|&element| element < floor)
    {
        *cursor = cursor.saturating_add(1);
    }
}

/// b1 and b2 (ITU-T T.6 §2.2.2): b1 is the first changing element on the
/// reference row past a0 of the colour opposite a0's, b2 the next one; an
/// absent element sits at the row's end. Even indices turn black, so b1 is
/// the first index from the cursor whose parity matches a0's colour.
#[inline]
fn reference_pair(
    reference: &[usize],
    cursor: &mut usize,
    floor: usize,
    white: bool,
    width: usize,
) -> (usize, usize) {
    advance(reference, cursor, floor);
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

/// The first changing element of the coding row at or past `floor`, or the
/// row's end.
fn next_change(changes: &[usize], cursor: &mut usize, floor: usize, width: usize) -> usize {
    advance(changes, cursor, floor);
    match changes.get(*cursor) {
        Some(&element) => element,
        None => width,
    }
}

/// Decodes one 2D row against `reference`, the row above's changing
/// elements, painting it and recording its own into `changes`.
pub(crate) fn decode_2d_row(
    reader: &mut BitReader<'_>,
    lookups: Lookups,
    reference: &[usize],
    changes: &mut RowChanges<'_>,
) -> Result<(), Corrupt> {
    let width = changes.at().width();
    let modes = lookups.mode_slots();
    let (mut start, mut floor) = (0_usize, 0_usize);
    let mut white = true;
    let mut cursor: usize = 0;
    while start < width {
        let (b1, b2) = reference_pair(reference, &mut cursor, floor, white, width);
        match read_mode(reader, modes)? {
            Mode::Pass => {
                changes.paint((start, b2), white)?;
                start = b2;
            }
            Mode::Horizontal => {
                let a1 = start.saturating_add(decode_run(reader, lookups.run_slots(white), width)?);
                changes.paint((start, a1), white)?;
                let a2 = a1.saturating_add(decode_run(reader, lookups.run_slots(!white), width)?);
                changes.paint((a1, a2), !white)?;
                start = a2;
            }
            Mode::Vertical(offset) => {
                let a1 = match b1.checked_add_signed(offset) {
                    Some(a1) if a1 >= floor => a1,
                    _ => return cut(),
                };
                changes.paint((start, a1), white)?;
                start = a1;
                white = !white;
            }
        }
        floor = start.saturating_add(1);
    }
    Ok(())
}

/// The row above's changing elements and the row being decoded's, swapped
/// row after row. One is made per page and both buffers are sized once, so
/// recording a change is a bounds-checked store with no growth, and a
/// page of single-row strips allocates twice, not twice per strip.
///
/// A row of `width` pixels has at most `width` changing elements, and a
/// row of one strip has no more than that strip has bits, since every
/// change costs a code bit or more; the page's capacity is the smaller of
/// the two plus one, so a corrupt row that would need more is refused.
pub(crate) struct Reference {
    /// The changing elements of the row above, in its first `above_len`.
    above: Vec<usize>,
    /// How many elements of `above` the row above holds.
    above_len: usize,
    /// The changing elements recorded for the row being decoded.
    current: Vec<usize>,
}

impl Reference {
    /// Room for `capacity` changing elements per row.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            above: vec![0; capacity],
            above_len: 0,
            current: vec![0; capacity],
        }
    }

    /// The white row before a strip: no changing elements.
    pub(crate) const fn restart(&mut self) {
        self.above_len = 0;
    }

    /// The row above's changes, and a painter for `row` of `strip` that
    /// records the row's own changes for the row after it.
    pub(crate) fn begin_row<'row>(
        &'row mut self,
        strip: &'row mut Strip<'_>,
        row: usize,
    ) -> Result<(&'row [usize], RowChanges<'row>), TiffError> {
        let (window, at) = strip.row_window(row)?;
        let above = match self.above.get(..self.above_len) {
            Some(held) => held,
            None => &[],
        };
        Ok((
            above,
            RowChanges {
                out: &mut self.current,
                count: 0,
                black: false,
                window,
                at,
            },
        ))
    }

    /// Makes the row just decoded, with its `count` changes, the reference
    /// of the row after it.
    pub(crate) const fn roll(&mut self, count: usize) {
        core::mem::swap(&mut self.above, &mut self.current);
        self.above_len = count;
    }
}

/// Paints one row's runs and records its changing elements as it goes.
///
/// The page bitmap starts zeroed and a decoded row's runs cover it left to
/// right without a gap, so a changing element is exactly where a non-empty
/// run's colour differs from the run before it, the pixel before the row
/// counting as white. That is the reference the next row needs, read from
/// the decode instead of scanning the bitmap back. The row's bytes are
/// taken once, so a run is a bounds check and a fill.
pub(crate) struct RowChanges<'row> {
    /// Room for the row's changing elements, ascending, each below the
    /// width; the first `count` are recorded.
    out: &'row mut [usize],
    /// How many changing elements the row has so far.
    count: usize,
    /// The colour of the last non-empty run.
    black: bool,
    /// The packed bytes of the row being decoded.
    window: &'row mut [u8],
    /// Where the row's decode stands.
    at: RowAt,
}

impl RowChanges<'_> {
    /// Where the row's decode stands.
    #[inline]
    pub(crate) const fn at(&self) -> RowAt {
        self.at
    }

    /// How many changing elements the row has recorded.
    pub(crate) const fn recorded(&self) -> usize {
        self.count
    }

    /// Paints `from..to` in one colour (black sets bits, white leaves the
    /// zeroed row alone) and records the change it makes, if any. Refuses a
    /// span that runs backwards or past the row, and a change past the
    /// room the page sized. Always inlined: it runs for every span, and as
    /// a call it paid a full register save each time.
    #[inline(always)]
    pub(crate) fn paint(&mut self, span: (usize, usize), white: bool) -> Result<(), Corrupt> {
        let (from, to) = span;
        if from > to || to > self.at.width() {
            return cut();
        }
        if from < to && white == self.black {
            let Some(slot) = self.out.get_mut(self.count) else {
                return cut();
            };
            *slot = from;
            self.count = self.count.saturating_add(1);
            self.black = !white;
        }
        if !white {
            set_span(self.window, from, to);
        }
        Ok(())
    }
}

/// Decodes one G4 strip: every row in 2D, the first against a white row.
pub(crate) fn decode_g4_strip(
    data: &[u8],
    strip: &mut Strip<'_>,
    reference: &mut Reference,
    limits: &Limits,
) -> Result<(), TiffError> {
    let mut reader = BitReader::new(data);
    let lookups = Lookups::new();
    reference.restart();
    for row in strip.rows() {
        limits.check_records(row.saturating_add(1))?;
        let (above, mut changes) = reference.begin_row(strip, row)?;
        let decoded = decode_2d_row(&mut reader, lookups, above, &mut changes);
        let count = changes.recorded();
        changes.at().as_row_refusal(decoded)?;
        reference.roll(count);
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
    let (mut start, mut floor) = (0_usize, 0_usize);
    let mut white = true;
    let (mut coding_cursor, mut reference_cursor) = (0_usize, 0_usize);
    while start < width {
        let a1 = next_change(coding, &mut coding_cursor, floor, width);
        let (b1, b2) = reference_pair(reference, &mut reference_cursor, floor, white, width);
        if b2 < a1 {
            writer.push_code(MODE_PASS);
            start = b2;
        } else if let Some((&code, _)) = MODE_VERTICAL
            .iter()
            .zip(VERTICAL_OFFSETS.iter())
            .find(|pair| b1.checked_add_signed(*pair.1) == Some(a1))
        {
            writer.push_code(code);
            start = a1;
            white = !white;
        } else {
            let mut lookahead = coding_cursor;
            let a2 = next_change(coding, &mut lookahead, a1.saturating_add(1), width);
            writer.push_code(MODE_HORIZ);
            emit_run(writer, a1.saturating_sub(start), white, page)?;
            emit_run(writer, a2.saturating_sub(a1), !white, page)?;
            start = a2;
        }
        floor = start.saturating_add(1);
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
    use super::{Reference, decode_g4_strip};
    use crate::legacy::limits::Limits;

    /// Decodes `stream` as one G4 row `width` pixels wide.
    fn one_row(stream: &[u8], width: usize) -> Result<Vec<u8>, TiffError> {
        let row_bytes = width.div_ceil(8);
        let mut bitmap = vec![0_u8; row_bytes];
        let mut strip = Strip::new(&mut bitmap, (row_bytes, width), 0..1, 0);
        let mut reference = Reference::new(width.saturating_add(1));
        let bounds = Limits::new(4096, 64, 64, 8, 4096);
        decode_g4_strip(stream, &mut strip, &mut reference, &bounds)?;
        Ok(bitmap)
    }

    /// A page sized for fewer changes than a row holds refuses the row
    /// that needs more, at that row, instead of growing or writing past;
    /// with room the same stream decodes. `001 00110101 010 1` is H W0 B1
    /// then V0 to the row's end (ITU-T T.6 Table 1, T.4 Table 2): changes
    /// at pixels 0 and 1.
    #[test]
    fn a_row_past_the_sized_room_is_refused_at_its_row() -> Result<(), TiffError> {
        let stream = [0b0010_0110, 0b1010_1010];
        let bounds = Limits::new(4096, 64, 64, 8, 4096);
        for (room, page) in [(1_usize, 3_usize), (9, 4)] {
            let mut bitmap = vec![0_u8; 1];
            let mut strip = Strip::new(&mut bitmap, (1, 8), 0..1, page);
            let decoded = decode_g4_strip(&stream, &mut strip, &mut Reference::new(room), &bounds);
            if room == 1 {
                assert_eq!(
                    decoded,
                    Err(TiffError::CorruptStrip { page, row: 0 }),
                    "two changes in room for one must be refused at row 0 of page {page}"
                );
            } else {
                decoded?;
                assert_eq!(bitmap, vec![0b1000_0000], "with room the row is B1 W7");
            }
        }
        Ok(())
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
