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
    EOL_BITS, EOL_LEN, EXTENDED_MAKEUP, MAX_MAKEUP, RUN_LOOKUP_BITS, colour_tables, run_lookup_for,
};
use super::{Strip, TiffError, pixel_at, row_changes};

/// A bit reader over strip bytes, MSB first, holding only a bit position:
/// a code is peeked through a lookup and then skipped, and a reader that
/// looked too far rewinds to a position it saved, with no pushback buffer.
pub(crate) struct BitReader<'a> {
    /// The strip bytes.
    bytes: &'a [u8],
    /// The next bit to read, counted from the strip's first bit.
    position: usize,
}

impl<'a> BitReader<'a> {
    /// Reads strip bytes MSB first.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// The bits not yet read.
    fn left(&self) -> usize {
        self.bytes
            .len()
            .saturating_mul(8)
            .saturating_sub(self.position)
    }

    /// The next bit, or `None` past the end.
    pub(crate) fn next_bit(&mut self) -> Option<bool> {
        let (at, mask) = pixel_at(self.position);
        let &byte = self.bytes.get(at)?;
        self.position = self.position.saturating_add(1);
        Some(byte & mask != 0)
    }

    /// The next `count` bits (at most 16), most significant first, with the
    /// bits past the strip read as zeros, and how many of them the strip
    /// really holds. Nothing is consumed: a caller judges a code by the
    /// held count before it skips the code.
    ///
    /// Away from the strip's end one big-endian load of eight bytes holds
    /// every bit a peek can want; only the last bytes take the byte-wise
    /// path that reads zeros past the end.
    pub(crate) fn peek(&self, count: u8) -> (u32, usize) {
        let at = self.position.saturating_div(8);
        let window = match self
            .bytes
            .get(at..at.saturating_add(8))
            .and_then(|eight| <[u8; 8]>::try_from(eight).ok())
        {
            Some(eight) => u64::from_be_bytes(eight),
            None => {
                let mut eight = [0_u8; 8];
                if let Some(tail) = self.bytes.get(at..) {
                    for (slot, &byte) in eight.iter_mut().zip(tail) {
                        *slot = byte;
                    }
                }
                u64::from_be_bytes(eight)
            }
        };
        let [used, ..] = self.position.rem_euclid(8).to_le_bytes();
        let wanted = u32::from(count.min(16));
        let aligned = window.wrapping_shl(u32::from(used));
        let [.., high, low] = aligned.wrapping_shr(48).to_be_bytes();
        let top = u32::from(u16::from_be_bytes([high, low]));
        let bits = top.wrapping_shr(16_u32.saturating_sub(wanted)) & !u32::MAX.wrapping_shl(wanted);
        (bits, self.left().min(usize::from(count)))
    }

    /// Consumes `count` bits a peek has judged.
    pub(crate) fn skip(&mut self, count: u8) {
        self.position = self.position.saturating_add(usize::from(count));
    }

    /// The position a later [`Self::rewind`] returns to.
    pub(crate) const fn mark(&self) -> usize {
        self.position
    }

    /// Returns to a position [`Self::mark`] saved.
    pub(crate) const fn rewind(&mut self, mark: usize) {
        self.position = mark;
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
pub(crate) fn decode_run(
    reader: &mut BitReader<'_>,
    white: bool,
    strip: &Strip<'_>,
    row: usize,
) -> Result<usize, TiffError> {
    let table = run_lookup_for(white);
    let mut run: usize = 0;
    loop {
        let (bits, held) = reader.peek(RUN_LOOKUP_BITS);
        let slot = usize::try_from(bits)
            .ok()
            .and_then(|index| table.get(index).copied());
        let Some(slot) = slot else {
            return crate::legacy::refuse(strip.corrupt(row));
        };
        let len = slot.len();
        if len == 0 || usize::from(len) > held {
            return crate::legacy::refuse(strip.corrupt(row));
        }
        reader.skip(len);
        run = run.saturating_add(slot.value());
        if !slot.is_makeup() {
            return Ok(run);
        }
        if run > strip.width() {
            return crate::legacy::refuse(strip.corrupt(row));
        }
    }
}

/// Skips EOL codes and the fill zeros before each, returning how many were
/// skipped. A one after fewer than eleven zeros is row data, and the reader
/// rewinds to before its zeros.
pub(crate) fn skip_eols(reader: &mut BitReader<'_>) -> usize {
    let mut skipped: usize = 0;
    loop {
        let mark = reader.mark();
        let mut zeros: usize = 0;
        loop {
            match reader.next_bit() {
                Some(false) => zeros = zeros.saturating_add(1),
                Some(true) => break,
                None => {
                    if zeros < usize::from(EOL_LEN) {
                        reader.rewind(mark);
                    }
                    return skipped;
                }
            }
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
    strip: &mut Strip<'_>,
    row: usize,
    changes: &mut t6::RowChanges<'_>,
) -> Result<(), TiffError> {
    let mut x: usize = 0;
    let mut white = true;
    while x < strip.width() {
        let run = decode_run(reader, white, strip, row)?;
        let end = x.saturating_add(run);
        changes.paint(strip, row, (x, end), white)?;
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
    let mut reference = t6::Reference::new();
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
        let (above, mut changes) = reference.begin_row();
        if coded_2d {
            t6::decode_2d_row(&mut reader, strip, row, above, &mut changes)?;
        } else {
            decode_1d_row(&mut reader, strip, row, &mut changes)?;
        }
        reference.roll();
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
