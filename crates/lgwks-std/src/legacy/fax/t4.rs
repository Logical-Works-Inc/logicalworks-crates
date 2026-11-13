//! T.4 run coding: G3 strips (ITU-T T.4 §4.1, §4.2), and the bit reader
//! and writer both coders share.
//!
//! [`decode_g3_strip`] turns strip bytes into bitmap rows: one-dimensional
//! rows, or under TIFF `T4Options` bit 0 a tag bit after every EOL choosing a
//! 1D row or a 2D row coded against the row above. EOLs may carry fill bits.
//! [`encode_g3`] writes one continuous 1D stream: an EOL before every row,
//! then the return-to-control of six EOLs. A run code resolves in one index
//! of its colour's lookup, built from the T.4 tables, instead of a table
//! scan per bit.

use super::super::limits::Limits;
use super::t6;
use super::tables::{
    EOL_BITS, EOL_LEN, EXTENDED_MAKEUP, Lookups, MAX_MAKEUP, RUN_LOOKUP_BITS, Slot, colour_tables,
};
use super::{Corrupt, Strip, TiffError, cut, pixel_at, row_changes};

/// A bit reader over strip bytes, MSB first. The next bits wait in a 64-bit
/// accumulator, the next bit at the top and zeros below the bits loaded, so
/// a code is peeked with one shift and skipped with another, and the strip
/// is loaded eight bytes at a time, not once per code. A reader that looked
/// too far rewinds to a bit position it saved.
pub(crate) struct BitReader<'a> {
    /// The strip bytes.
    bytes: &'a [u8],
    /// The first byte not yet loaded into `acc`.
    next: usize,
    /// The loaded bits not yet read, from the most significant bit down;
    /// every bit below the `held` top bits is zero.
    acc: u64,
    /// How many of `acc`'s top bits are strip bits: at most 64.
    held: u8,
    /// The next bit to read, counted from the strip's first bit.
    position: usize,
}

impl<'a> BitReader<'a> {
    /// Reads strip bytes MSB first.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            next: 0,
            acc: 0,
            held: 0,
            position: 0,
        }
    }

    /// Loads the whole bytes that fit behind the held bits. Away from the
    /// strip's end that is one big-endian load of eight bytes, masked to the
    /// bytes that fit; the last bytes come one at a time, and nothing past
    /// the end is loaded, so the bits past it read as zeros.
    ///
    /// Kept out of line: it runs about once per 48 bits, and inlined into
    /// every code read it crowded the 2D row decoder's registers (G4 decode
    /// measured 5% slower with it inline).
    #[inline(never)]
    fn refill(&mut self) {
        let [room, ..] = 64_u8
            .saturating_sub(self.held)
            .saturating_div(8)
            .to_le_bytes();
        let rest = self.bytes.get(self.next..);
        if let Some(&eight) = rest.and_then(<[u8]>::first_chunk::<8>) {
            let keep = self.held.saturating_add(room.saturating_mul(8));
            let fits = match u64::MAX.checked_shr(u32::from(keep)) {
                Some(below) => !below,
                None => u64::MAX,
            };
            let word = u64::from_be_bytes(eight).wrapping_shr(u32::from(self.held));
            self.acc |= word & fits;
            self.held = keep;
            self.next = self.next.saturating_add(usize::from(room));
            return;
        }
        for &byte in rest.into_iter().flatten().take(usize::from(room)) {
            self.acc |= u64::from(byte).wrapping_shl(u32::from(56_u8.saturating_sub(self.held)));
            self.held = self.held.saturating_add(8);
            self.next = self.next.saturating_add(1);
        }
    }

    /// The next bit, or `None` past the end.
    pub(crate) fn next_bit(&mut self) -> Option<bool> {
        let (bit, held) = self.peek(1);
        if held == 0 {
            return None;
        }
        self.skip(1);
        Some(bit == 1)
    }

    /// The next `count` bits (at most 16), most significant first, with the
    /// bits past the strip read as zeros, and how many of them the strip
    /// really holds. Nothing is consumed: a caller judges a code by the
    /// held count before it skips the code.
    #[inline]
    pub(crate) fn peek(&mut self, count: u8) -> (u32, usize) {
        if self.held < 16 {
            self.refill();
        }
        let wanted = u32::from(count.clamp(1, 16));
        let top = self.acc.wrapping_shr(64_u32.saturating_sub(wanted));
        let [.., high, low] = top.to_be_bytes();
        let bits = u32::from(u16::from_be_bytes([high, low]));
        (bits, usize::from(self.held.min(count)))
    }

    /// Consumes `count` bits a peek has judged, never more than that peek
    /// held: every caller skips a code its peek found whole, and the bits
    /// past the strip's end are not there to consume.
    #[inline]
    pub(crate) fn skip(&mut self, count: u8) {
        let taken = count.min(self.held);
        self.acc = self.acc.wrapping_shl(u32::from(taken));
        self.held = self.held.saturating_sub(taken);
        self.position = self.position.saturating_add(usize::from(taken));
    }

    /// Reads one code through `table`, indexed by the next `lookup_bits`
    /// bits, and consumes it. The codes of one table are prefix-free, so the
    /// slot names the one code those bits start with. No code there is
    /// corrupt, and so is a code longer than the bits the strip still holds:
    /// the peek reads zeros past the end, which must never complete a code.
    #[inline]
    pub(crate) fn read_slot(&mut self, table: &[Slot], lookup_bits: u8) -> Result<Slot, Corrupt> {
        let (bits, held) = self.peek(lookup_bits);
        let slot = usize::try_from(bits)
            .ok()
            .and_then(|index| table.get(index).copied());
        let Some(slot) = slot else {
            return cut();
        };
        let len = slot.len();
        if len == 0 || usize::from(len) > held {
            return cut();
        }
        self.skip(len);
        Ok(slot)
    }

    /// The position a later [`Self::rewind`] returns to.
    pub(crate) const fn mark(&self) -> usize {
        self.position
    }

    /// Returns to a position [`Self::mark`] saved: the accumulator is
    /// emptied and refilled from that bit's byte, its earlier bits dropped.
    pub(crate) fn rewind(&mut self, mark: usize) {
        self.position = mark;
        self.next = mark.saturating_div(8);
        self.acc = 0;
        self.held = 0;
        let [used, ..] = mark.rem_euclid(8).to_le_bytes();
        if used == 0 {
            return;
        }
        self.refill();
        // A mark past the strip's end loads nothing, and drops nothing.
        let dropped = used.min(self.held);
        self.acc = self.acc.wrapping_shl(u32::from(dropped));
        self.held = self.held.saturating_sub(dropped);
    }
}

/// A bit writer for strip bytes, MSB first.
pub(crate) struct BitWriter {
    /// The finished bytes.
    out: Vec<u8>,
    /// The open byte.
    current: u8,
    /// Bits filled in the open byte.
    filled: u8,
}

impl BitWriter {
    /// An empty writer.
    pub(crate) const fn new() -> Self {
        Self {
            out: Vec::new(),
            current: 0,
            filled: 0,
        }
    }

    /// Pushes one bit.
    fn push_bit(&mut self, bit: bool) {
        if bit {
            let (_, mask) = pixel_at(usize::from(self.filled));
            self.current |= mask;
        }
        self.filled = self.filled.saturating_add(1);
        if self.filled >= 8 {
            self.out.push(self.current);
            self.current = 0;
            self.filled = 0;
        }
    }

    /// Pushes one `(bits, length)` code, most significant bit first.
    pub(crate) fn push_code(&mut self, code: (u16, u8)) {
        let (bits, len) = code;
        let mut remaining = u32::from(len);
        while remaining > 0 {
            remaining = remaining.saturating_sub(1);
            let bit = bits
                .checked_shr(remaining)
                .is_some_and(|moved| moved & 1 == 1);
            self.push_bit(bit);
        }
    }

    /// The finished bytes with the open byte padded by zeros.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.out.push(self.current);
        }
        self.out
    }
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
/// Each colour's code set is prefix-free, so the slot the next 13 bits
/// index names the one code they start with. No code there is corrupt, and
/// so is a code longer than the bits the strip still holds (the peek reads
/// zeros past the end, which must never complete a code) and a run past
/// the row.
#[inline]
pub(crate) fn decode_run(
    reader: &mut BitReader<'_>,
    table: &[Slot],
    width: usize,
) -> Result<usize, Corrupt> {
    let mut run: usize = 0;
    loop {
        let slot = reader.read_slot(table, RUN_LOOKUP_BITS)?;
        run = run.saturating_add(slot.value());
        if !slot.is_makeup() {
            return Ok(run);
        }
        if run > width {
            return cut();
        }
    }
}

/// Counts the zeros from the reader's position up to the next one bit,
/// consuming them and that one, and whether a one ended them: when the
/// strip ends first, every zero to its end is consumed. Sixteen bits are
/// judged per peek, where a bit at a time cost an EOL twelve reads.
fn zeros_to_one(reader: &mut BitReader<'_>) -> (usize, bool) {
    let mut zeros: usize = 0;
    loop {
        let (bits, held) = reader.peek(16);
        let [lead, ..] = bits.leading_zeros().saturating_sub(16).to_le_bytes();
        if usize::from(lead) < held {
            reader.skip(lead.saturating_add(1));
            return (zeros.saturating_add(usize::from(lead)), true);
        }
        if held == 0 {
            return (zeros, false);
        }
        let [step, ..] = held.to_le_bytes();
        reader.skip(step);
        zeros = zeros.saturating_add(held);
    }
}

/// Skips EOL codes and the fill zeros before each, returning how many were
/// skipped. A one after fewer than eleven zeros is row data, and the reader
/// rewinds to before its zeros.
pub(crate) fn skip_eols(reader: &mut BitReader<'_>) -> usize {
    let mut skipped: usize = 0;
    loop {
        let mark = reader.mark();
        let (zeros, ended_on_one) = zeros_to_one(reader);
        if !ended_on_one {
            if zeros < usize::from(EOL_LEN) {
                reader.rewind(mark);
            }
            return skipped;
        }
        if zeros >= usize::from(EOL_LEN.saturating_sub(1)) {
            skipped = skipped.saturating_add(1);
            continue;
        }
        reader.rewind(mark);
        return skipped;
    }
}

/// Decodes one 1D row: runs alternating from white until the row is full,
/// recording the row's changing elements into `changes`.
fn decode_1d_row(
    reader: &mut BitReader<'_>,
    lookups: Lookups,
    changes: &mut t6::RowChanges<'_>,
) -> Result<(), Corrupt> {
    let width = changes.at().width();
    let mut x: usize = 0;
    let mut white = true;
    while x < width {
        let run = decode_run(reader, lookups.run_slots(white), width)?;
        let end = x.saturating_add(run);
        changes.paint((x, end), white)?;
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
    reference: &mut t6::Reference,
    two_d: bool,
    limits: &Limits,
) -> Result<(), TiffError> {
    let mut reader = BitReader::new(data);
    let lookups = Lookups::new();
    reference.restart();
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
        let (above, mut changes) = reference.begin_row(strip, row)?;
        let decoded = if coded_2d {
            t6::decode_2d_row(&mut reader, lookups, above, &mut changes)
        } else {
            decode_1d_row(&mut reader, lookups, &mut changes)
        };
        let count = changes.recorded();
        changes.at().as_row_refusal(decoded)?;
        reference.roll(count);
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

#[cfg(test)]
mod tests {
    use super::{BitReader, skip_eols};

    /// The oracle: the `count` bits at bit `at` of `bytes`, MSB first, zeros
    /// past the end, and how many of them the bytes hold.
    fn model_peek(bytes: &[u8], at: usize, count: u8) -> (u32, usize) {
        let mut bits: u32 = 0;
        let mut held: usize = 0;
        for offset in 0..usize::from(count) {
            let bit = at.saturating_add(offset);
            let set = bytes.get(bit.saturating_div(8)).map(|&byte| {
                let [shift, ..] = (7_usize.saturating_sub(bit.rem_euclid(8))).to_le_bytes();
                byte.wrapping_shr(u32::from(shift)) & 1 == 1
            });
            if set.is_some() {
                held = held.saturating_add(1);
            }
            bits = bits.wrapping_shl(1) | u32::from(set == Some(true));
        }
        (bits, held)
    }

    /// Seeded strips of 0 to 40 bytes read by random peeks of 1 to 16 bits,
    /// skips of what a peek held, rewinds to earlier marks and single bits,
    /// against a bit-by-bit model: every refill path (eight-byte load,
    /// partial bytes, the strip's last bytes, past the end) answers as the
    /// model does at every position.
    #[test]
    fn sim_the_accumulator_reads_every_bit_the_model_reads() -> Result<(), String> {
        for seed in 0..512_u64 {
            let mut stream = crate::seeded::Seeded::from_seed(seed);
            let mut draw = |below: u64| stream.below(below).map_err(|error| format!("{error}"));
            let len = usize::try_from(draw(41)?).map_err(|error| error.to_string())?;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                bytes.push(u8::try_from(draw(256)?).map_err(|error| error.to_string())?);
            }
            let mut reader = BitReader::new(&bytes);
            let mut marks: Vec<usize> = vec![0];
            for step in 0..200 {
                let at = reader.mark();
                let count =
                    u8::try_from(draw(16)?.saturating_add(1)).map_err(|error| error.to_string())?;
                match draw(8)? {
                    0 => {
                        let marked =
                            u64::try_from(marks.len()).map_err(|error| error.to_string())?;
                        let back =
                            usize::try_from(draw(marked)?).map_err(|error| error.to_string())?;
                        let mark = marks.get(back).copied().ok_or("no mark")?;
                        reader.rewind(mark);
                        assert_eq!(reader.mark(), mark, "seed {seed} step {step}: rewound");
                    }
                    1 => {
                        let (bit, held) = model_peek(&bytes, at, 1);
                        let expected = (held == 1).then_some(bit == 1);
                        assert_eq!(
                            reader.next_bit(),
                            expected,
                            "seed {seed} step {step}: next bit at {at}"
                        );
                    }
                    _ => {
                        let expected = model_peek(&bytes, at, count);
                        assert_eq!(
                            reader.peek(count),
                            expected,
                            "seed {seed} step {step}: peek {count} at {at}"
                        );
                        // The whole peek is skipped; near the end only the
                        // bits the strip held are consumed.
                        reader.skip(count);
                        assert_eq!(
                            reader.mark(),
                            at.saturating_add(expected.1),
                            "seed {seed} step {step}: skipped {count} of {} held",
                            expected.1
                        );
                        marks.push(reader.mark());
                    }
                }
            }
        }
        Ok(())
    }

    /// A rewind to a bit inside a byte reads from that bit; the strip's last
    /// bits come back with zeros, a skip consumes only the bits the strip
    /// held, and a rewind past the end holds nothing.
    #[test]
    fn a_rewind_mid_byte_reads_from_its_bit_and_a_skip_stops_at_the_end() {
        let mut reader = BitReader::new(&[0b0000_0101, 0b1100_0000]);
        reader.rewind(5);
        assert_eq!(reader.peek(4), (0b1011, 4), "bits 5 to 8");
        reader.skip(4);
        assert_eq!(
            reader.peek(16),
            (0b1000_0000_0000_0000, 7),
            "the last seven bits, then zeros"
        );
        reader.skip(16);
        assert_eq!(reader.mark(), 16, "only the seven held bits are consumed");
        assert_eq!(reader.peek(16), (0, 0), "nothing is left");
        assert_eq!(reader.next_bit(), None, "no bit past the end");
        reader.rewind(21);
        assert_eq!(reader.peek(8), (0, 0), "a mark past the end holds nothing");
    }

    /// ITU-T T.4 §4.1.2: an EOL is eleven zeros and a one, and fill zeros of
    /// any length may precede it (TIFF 6.0 Section 10, `T4Options` bit 2).
    /// Each layout puts the EOL's one, or the strip's end, at a peek edge.
    #[test]
    fn eol_fill_is_read_past_a_peek_and_zeros_to_the_end_are_no_eol() {
        // Twenty fill zeros, then the one: one EOL, read across two peeks.
        let mut reader = BitReader::new(&[0x00, 0x00, 0x08]);
        assert_eq!(
            skip_eols(&mut reader),
            1,
            "twenty zeros and a one is an EOL"
        );
        assert_eq!(reader.mark(), 21, "consumed through the EOL's one");
        // Sixteen zeros to the end: a full peek with no one is no EOL, and the
        // trailing fill is consumed.
        let mut reader = BitReader::new(&[0x00, 0x00]);
        assert_eq!(skip_eols(&mut reader), 0, "zeros to the end are no EOL");
        assert_eq!(reader.mark(), 16, "trailing fill as long as a peek");
        // Four data bits, then exactly an EOL's twelve bits of zeros to the
        // end: consumed as fill, not rewound as row data.
        let mut reader = BitReader::new(&[0xf0, 0x00]);
        reader.rewind(4);
        assert_eq!(skip_eols(&mut reader), 0, "twelve zeros to the end");
        assert_eq!(reader.mark(), 16, "twelve trailing zeros are fill");
        // Four data bits, then eleven zeros and a one on the strip's last bit:
        // an EOL, consumed through that one.
        let mut reader = BitReader::new(&[0xf0, 0x01]);
        reader.rewind(4);
        assert_eq!(skip_eols(&mut reader), 1, "eleven zeros and a one");
        assert_eq!(reader.mark(), 16, "consumed through that one");
        // Four data bits, then seven zeros and a one: row data, so the reader
        // rewinds to before the zeros.
        let mut reader = BitReader::new(&[0xf0, 0x10]);
        reader.rewind(4);
        assert_eq!(skip_eols(&mut reader), 0, "seven zeros and a one is data");
        assert_eq!(reader.mark(), 4, "rewound to the row data");
    }
}
