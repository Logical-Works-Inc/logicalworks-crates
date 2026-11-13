//! The packed page bitmap the fax coders read and paint: pixel masks, a
//! row's changing elements (ITU-T T.4 §4.2.1.3.1), span fills, the rows one
//! strip decodes into, and the marker a per-code path refuses with.
//!
//! A row is MSB first and padded to whole bytes; a set bit is black.

use super::TiffError;

/// The mask of each pixel within its byte, most significant first.
const PIXEL_MASK: [u8; 8] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];

/// The byte and mask of pixel `x` in a packed row.
pub(crate) fn pixel_at(x: usize) -> (usize, u8) {
    match PIXEL_MASK.get(x.rem_euclid(8)) {
        Some(&mask) => (x.saturating_div(8), mask),
        None => (x.saturating_div(8), 0),
    }
}

/// The changing elements of a packed row (ITU-T T.4 §4.2.1.3.1): every
/// position whose colour differs from the pixel before it, the pixel before
/// the row counting as white. Even indices turn black, odd indices white.
///
/// A byte at a time: the pixels from `x` to its byte's end are flipped to
/// the current colour's sense, so the leading zeros of what is left are the
/// run that colour still holds, and a byte of one colour is passed over
/// whole.
pub(crate) fn row_changes(row: &[u8], width: usize) -> Vec<usize> {
    let mut changes: Vec<usize> = Vec::new();
    let mut black = false;
    let mut x: usize = 0;
    while x < width {
        let byte = match row.get(x.saturating_div(8)) {
            Some(&value) => value,
            None => 0,
        };
        let [used, ..] = x.rem_euclid(8).to_le_bytes();
        let differing = if black { !byte } else { byte };
        let ahead = differing.wrapping_shl(u32::from(used));
        let span = usize::from(8_u8.saturating_sub(used));
        let [zeros, ..] = ahead.leading_zeros().to_le_bytes();
        let run = usize::from(zeros).min(span);
        let next = x.saturating_add(run);
        if run < span {
            if next >= width {
                break;
            }
            changes.push(next);
            black = !black;
        }
        x = next;
    }
    changes
}

/// Sets every byte of `bytes` to `0xff`. Most fax runs cover a few bytes,
/// where a call into libc's memset costs more than the fill (it was the
/// G4 decode's largest sample), so a short span is two overlapping
/// fixed-size stores, the first and last `k` bytes for the largest `k` of
/// 8, 4, 2 or 1 that fits; a long span still takes memset.
#[inline(always)]
fn fill_ones(bytes: &mut [u8]) {
    /// Writes ones over the first and last `K` bytes, which cover a span
    /// of `K..=2K` bytes.
    fn ends<const K: usize>(bytes: &mut [u8]) {
        let len = bytes.len();
        if let Some(head) = bytes.get_mut(..K) {
            head.copy_from_slice(&[0xff; K]);
        }
        if let Some(tail) = bytes.get_mut(len.saturating_sub(K)..) {
            tail.copy_from_slice(&[0xff; K]);
        }
    }
    match bytes.len() {
        0 => {}
        1 => ends::<1>(bytes),
        2..=3 => ends::<2>(bytes),
        4..=7 => ends::<4>(bytes),
        8..=16 => ends::<8>(bytes),
        _ => bytes.fill(0xff),
    }
}

/// Sets bits `from..to` of a packed row: a partial first byte, whole bytes
/// filled at once, then a partial last byte.
#[inline(always)]
pub(crate) fn set_span(window: &mut [u8], from: usize, to: usize) {
    if from >= to {
        return;
    }
    let first = from.saturating_div(8);
    let last = to.saturating_sub(1).saturating_div(8);
    let [head_skip, ..] = from.rem_euclid(8).to_le_bytes();
    let [tail_keep, ..] = to.saturating_sub(1).rem_euclid(8).to_le_bytes();
    let head = 0xff_u8.wrapping_shr(u32::from(head_skip));
    let tail = 0xff_u8.wrapping_shl(u32::from(7_u8.saturating_sub(tail_keep)));
    if first == last {
        if let Some(byte) = window.get_mut(first) {
            *byte |= head & tail;
        }
        return;
    }
    if let Some(byte) = window.get_mut(first) {
        *byte |= head;
    }
    if let Some(middle) = window.get_mut(first.saturating_add(1)..last) {
        fill_ones(middle);
    }
    if let Some(byte) = window.get_mut(last) {
        *byte |= tail;
    }
}

/// The page rows one strip decodes into: the bitmap, its geometry, and the
/// span of rows the strip covers.
pub(crate) struct Strip<'page> {
    /// The whole page bitmap, row after row.
    bitmap: &'page mut [u8],
    /// Bytes of one packed row.
    row_bytes: usize,
    /// Pixels in one row.
    width: usize,
    /// The page rows this strip covers.
    span: core::ops::Range<usize>,
    /// Zero-based page index, for errors.
    page: usize,
}

impl<'page> Strip<'page> {
    /// The strip covering the `span` rows of a page bitmap.
    pub(crate) const fn new(
        bitmap: &'page mut [u8],
        geometry: (usize, usize),
        span: core::ops::Range<usize>,
        page: usize,
    ) -> Self {
        let (row_bytes, width) = geometry;
        Self {
            bitmap,
            row_bytes,
            width,
            span,
            page,
        }
    }

    /// The page rows this strip covers.
    pub(crate) fn rows(&self) -> core::ops::Range<usize> {
        self.span.clone()
    }

    /// The typed refusal for a strip that fails at `row`.
    pub(crate) const fn corrupt(&self, row: usize) -> TiffError {
        TiffError::CorruptStrip {
            page: self.page,
            row,
        }
    }

    /// The packed bytes of `row`, taken once per row so painting a run is
    /// a bounds check and a fill, and where that row's decode stands.
    pub(crate) fn row_window(&mut self, row: usize) -> Result<(&mut [u8], RowAt), TiffError> {
        let at = RowAt {
            width: self.width,
            page: self.page,
            row,
        };
        let start = row.saturating_mul(self.row_bytes);
        match self
            .bitmap
            .get_mut(start..start.saturating_add(self.row_bytes))
        {
            Some(window) => Ok((window, at)),
            None => crate::legacy::refuse(at.corrupt()),
        }
    }
}

/// A row that does not decode, before it is named. The per-code paths
/// (a code read, a run, a painted span) return this empty marker, which
/// travels in a register where a [`TiffError`] travels through memory on
/// every `Ok` too; the row's decoder names the page and row once, as
/// [`TiffError::CorruptStrip`] through [`crate::legacy::refuse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Corrupt;

/// The corrupt branch of a per-code path: cold, so the loops that test it
/// keep their hot path straight.
#[cold]
#[inline(never)]
pub(crate) const fn cut<T>() -> Result<T, Corrupt> {
    Err(Corrupt)
}

/// Where one row's decode stands: the width that bounds its runs, and the
/// page and row a refusal names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowAt {
    /// Pixels in the row.
    width: usize,
    /// Zero-based page index.
    page: usize,
    /// Zero-based row index.
    row: usize,
}

impl RowAt {
    /// Pixels in the row.
    #[inline]
    pub(crate) const fn width(self) -> usize {
        self.width
    }

    /// The typed refusal for this row.
    const fn corrupt(self) -> TiffError {
        TiffError::CorruptStrip {
            page: self.page,
            row: self.row,
        }
    }

    /// Names a row decode's [`Corrupt`] as this row's refusal.
    pub(crate) fn as_row_refusal(self, decoded: Result<(), Corrupt>) -> Result<(), TiffError> {
        match decoded {
            Ok(()) => Ok(()),
            Err(Corrupt) => crate::legacy::refuse(self.corrupt()),
        }
    }
}
