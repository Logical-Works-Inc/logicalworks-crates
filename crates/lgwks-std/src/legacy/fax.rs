//! `fax` reads and writes bilevel TIFF images with CCITT fax compression.
//!
//! The TIFF baseline reader and writer (TIFF 6.0 Part 1, single- and
//! multi-page) carries 1-bit images whose strips hold raw bit-packed rows,
//! CCITT T.4 1D Huffman runs (G3, compression 3) or T.6 2D references (G4,
//! compression 4). Decoding yields a 1-bit MSB-first bitmap; encoding
//! writes the same bytes back, so `decode` then `encode` returns the same
//! bytes. Corrupt strips name the page and row; every other failure is a
//! typed error with its offset.
//!
//! The run tables live in `tables`, transcribed from ITU-T T.4 Tables 2, 3a
//! and 3b and T.6 Table 1. The unit tests prove each colour's code set
//! prefix-free, decode and re-encode known-answer streams worked by hand
//! from those tables (G3 1D, G3 2D under `T4Options`, G4 pass, vertical and
//! horizontal modes), and round-trip pages whose runs reach the extended
//! make-up codes.
//!
//! Layout: `tables` holds the T.4/T.6 code tables, `tiff` the TIFF container,
//! `t4` the G3 coder, `t6` the 2D coder G4 and G3 2D rows share, `bitmap`
//! the packed page rows they read and paint.

/// The packed page bitmap: pixels, changing elements, span fills, strips.
mod bitmap;
/// T.4 1D run coding (G3).
mod t4;
/// T.6 2D reference coding (G4).
mod t6;
/// T.4/T.6 code tables.
mod tables;
/// TIFF container: headers, IFDs, strips and pages.
mod tiff;

use std::fmt;

use super::limits::LimitError;

use self::bitmap::{Corrupt, RowAt, Strip, cut, pixel_at, row_changes, set_span};
use self::tiff as container;

/// The compression a page carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compression {
    /// Raw bit-packed rows.
    Raw,
    /// CCITT T.4 1D Huffman runs.
    G3,
    /// CCITT T.6 2D references.
    G4,
}

/// One bilevel image: width, height and an MSB-first 1-bit bitmap.
///
/// A set bit is a black mark. Rows pack left to right, each row padded to a
/// whole byte.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TiffImage {
    /// Image width in pixels.
    width: u32,
    /// Image height in rows.
    height: u32,
    /// Bytes of one packed row.
    row_bytes: usize,
    /// The packed bitmap, row after row.
    bitmap: Vec<u8>,
}

impl TiffImage {
    /// Builds an image from packed rows, checking the byte count. Bits past
    /// `width` in each row's last byte are cleared, so [`Self::bitmap`] is the
    /// canonical page a decode of its encoding returns.
    ///
    /// # Errors
    ///
    /// [`TiffError::LengthMismatch`] when the bitmap is not exactly
    /// `height` rows of `width` bits.
    pub fn from_bitmap(width: u32, height: u32, bitmap: &[u8]) -> Result<Self, TiffError> {
        let width_bits = match usize::try_from(width) {
            Ok(drawn) => drawn,
            Err(_) => {
                return crate::legacy::refuse(TiffError::LengthMismatch {
                    page: 0,
                    need: 1,
                    have: 0,
                });
            }
        };
        let row_bytes = width_bits.div_ceil(8);
        let rows = match usize::try_from(height) {
            Ok(drawn) => drawn,
            Err(_) => {
                return crate::legacy::refuse(TiffError::LengthMismatch {
                    page: 0,
                    need: 1,
                    have: 0,
                });
            }
        };
        if bitmap.len() != rows.saturating_mul(row_bytes) {
            return crate::legacy::refuse(TiffError::LengthMismatch {
                page: 0,
                need: rows.saturating_mul(row_bytes),
                have: bitmap.len(),
            });
        }
        let mut canonical = bitmap.to_vec();
        tiff::clear_padding(&mut canonical, row_bytes, width_bits);
        Ok(Self {
            width,
            height,
            row_bytes,
            bitmap: canonical,
        })
    }

    /// Image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in rows.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The stride between rows in [`Self::bitmap`]: `width` rounded up to
    /// whole bytes, most significant bit first.
    #[must_use]
    pub const fn row_bytes(&self) -> usize {
        self.row_bytes
    }

    /// The packed bitmap, row after row.
    #[must_use]
    pub fn bitmap(&self) -> &[u8] {
        &self.bitmap
    }

    /// One packed row, or `None` past the height.
    #[must_use]
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        let Ok(index) = usize::try_from(y) else {
            return None;
        };
        let rows = match usize::try_from(self.height) {
            Ok(drawn) => drawn,
            Err(_) => return None,
        };
        if index >= rows {
            return None;
        }
        let at = index.saturating_mul(self.row_bytes);
        self.bitmap.get(at..at.saturating_add(self.row_bytes))
    }
}

/// Why a TIFF image could not be decoded or encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TiffError {
    /// The first bytes are not a TIFF header.
    BadHeader,
    /// The input ends before the structure does.
    Truncated {
        /// Byte offset where the input ended.
        offset: usize,
        /// Bytes the structure needed.
        need: usize,
    },
    /// A required tag is missing or malformed.
    BadTag {
        /// Zero-based page index.
        page: usize,
        /// The tag number.
        tag: u16,
    },
    /// A shape the codec does not implement.
    Unsupported {
        /// Zero-based page index.
        page: usize,
        /// What is not implemented.
        what: String,
    },
    /// A strip that does not decode: names its page and row.
    CorruptStrip {
        /// Zero-based page index.
        page: usize,
        /// Zero-based row index where decoding failed.
        row: usize,
    },
    /// A bitmap that is not exactly height rows of width bits.
    LengthMismatch {
        /// Zero-based page index.
        page: usize,
        /// Bytes the shape needs.
        need: usize,
        /// Bytes supplied.
        have: usize,
    },
    /// The input or output passed a [`Limits`](super::limits::Limits) ceiling.
    Limit(LimitError),
}

impl fmt::Display for TiffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadHeader => formatter.write_str("not a TIFF header"),
            Self::Truncated { offset, need } => {
                write!(formatter, "TIFF ends at {offset} short of {need} bytes")
            }
            Self::BadTag { page, tag } => {
                write!(
                    formatter,
                    "TIFF page {page} tag {tag} is missing or malformed"
                )
            }
            Self::Unsupported { page, ref what } => {
                write!(formatter, "TIFF page {page} uses unsupported {what}")
            }
            Self::CorruptStrip { page, row } => {
                write!(formatter, "TIFF page {page} strip fails at row {row}")
            }
            Self::LengthMismatch { page, need, have } => write!(
                formatter,
                "TIFF page {page} bitmap holds {have} bytes for {need}"
            ),
            Self::Limit(ref cause) => write!(formatter, "TIFF limits refused the input: {cause}"),
        }
    }
}

crate::legacy::limits::limit_carrying_error!(TiffError);

/// Decodes every page of a TIFF image.
///
/// # Errors
///
/// [`TiffError`] for a bad header, truncation, tag, shape or corrupt strip,
/// or [`TiffError::Limit`] past a ceiling.
pub fn decode_tiff(
    bytes: &[u8],
    limits: &super::limits::Limits,
) -> Result<Vec<TiffImage>, TiffError> {
    container::decode_tiff(bytes, limits)
}

/// Encodes images into one TIFF: headers, IFDs, strips and data.
///
/// # Errors
///
/// [`TiffError`] for an oversize shape, or [`TiffError::Limit`] past a ceiling.
pub fn encode_tiff(
    images: &[TiffImage],
    compression: Compression,
    limits: &super::limits::Limits,
) -> Result<Vec<u8>, TiffError> {
    container::encode_tiff(images, compression, limits)
}

#[cfg(test)]
mod tests {
    use super::t4::BitWriter;
    use super::tables::{
        BLACK_MAKEUP, BLACK_TERM, EOL_BITS, EOL_LEN, EXTENDED_MAKEUP, MODE_HORIZ, MODE_PASS,
        MODE_VERTICAL, WHITE_MAKEUP, WHITE_TERM,
    };
    use super::{
        Compression, Strip, TiffError, TiffImage, decode_tiff, encode_tiff, set_span, t4, t6,
    };
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), TiffError>;

    /// A 16-pixel checkerboard row pair.
    const CHECKER: [u8; 4] = [0b1010_1010, 0b1010_1010, 0b0101_0101, 0b0101_0101];

    /// Encodes one page and decodes it back, requiring the page exactly.
    fn round_trips(image: &TiffImage, compression: Compression) -> UnitResult {
        let bytes = encode_tiff(core::slice::from_ref(image), compression, &BOUNDS)?;
        let back = decode_tiff(&bytes, &BOUNDS)?;
        assert_eq!(
            back.as_slice(),
            core::slice::from_ref(image),
            "{compression:?} must return the page exactly"
        );
        Ok(())
    }

    /// Decodes one strip of `rows` rows `width` wide with the given decoder.
    fn decode_strip(
        width: usize,
        rows: usize,
        decode: impl FnOnce(&mut Strip<'_>, &mut t6::Reference) -> UnitResult,
    ) -> Result<Vec<u8>, TiffError> {
        let row_bytes = width.div_ceil(8);
        let mut bitmap = vec![0_u8; row_bytes.saturating_mul(rows)];
        let mut strip = Strip::new(&mut bitmap, (row_bytes, width), 0..rows, 0);
        decode(&mut strip, &mut t6::Reference::new(width.saturating_add(1)))?;
        Ok(bitmap)
    }

    #[test]
    fn every_compression_round_trips_a_checkerboard() -> UnitResult {
        let image = TiffImage::from_bitmap(16, 2, &CHECKER)?;
        for compression in [Compression::Raw, Compression::G3, Compression::G4] {
            round_trips(&image, compression)?;
        }
        Ok(())
    }

    /// Rows whose runs need repeated 2560, extended and colour make-up codes,
    /// and whose 2D coding takes vertical, horizontal and pass modes.
    #[test]
    fn long_runs_and_every_2d_mode_round_trip() -> UnitResult {
        let (width, height) = (3000_usize, 5_usize);
        let row_bytes = width.div_ceil(8);
        let mut bitmap = vec![0_u8; row_bytes.saturating_mul(height)];
        for (row, from, to) in [(1, 0, 2700), (2, 1, 2699), (3, 1795, 1800), (3, 2990, 3000)] {
            let at = row_bytes.saturating_mul(row);
            if let Some(window) = bitmap.get_mut(at..at.saturating_add(row_bytes)) {
                set_span(window, from, to);
            }
        }
        let image = TiffImage::from_bitmap(3000, 5, &bitmap)?;
        for compression in [Compression::Raw, Compression::G3, Compression::G4] {
            round_trips(&image, compression)?;
        }
        Ok(())
    }

    /// Known answers worked by hand from ITU-T T.6 Table 1 and T.4 Table 2,
    /// eight pixels wide: rows `0011_0000`, `1000_0000` code as H W2 B2 V0,
    /// then VL2 VL3 P V0 (b1 matched by colour), then the EOFB.
    #[test]
    fn g4_known_answers_decode_and_encode() -> UnitResult {
        let stream = [0x2f, 0xc2, 0x04, 0x30, 0x01, 0x00, 0x10];
        let bitmap = decode_strip(8, 2, |strip, reference| {
            t6::decode_g4_strip(&stream, strip, reference, &BOUNDS)
        })?;
        assert_eq!(
            bitmap,
            vec![0x30, 0x80],
            "the colour-matched b1 stream must decode"
        );
        let rows: [&[u8]; 2] = [&[0x30], &[0x80]];
        assert_eq!(
            t6::encode_g4(&rows, 8, 0)?,
            stream,
            "the encoder must choose the same modes"
        );
        let pass_stream = [0x37, 0xf8, 0xc0, 0x04, 0x00, 0x40];
        let bitmap = decode_strip(8, 3, |strip, reference| {
            t6::decode_g4_strip(&pass_stream, strip, reference, &BOUNDS)
        })?;
        assert_eq!(
            bitmap,
            vec![0x0c, 0x0c, 0x00],
            "V0 runs and a pass must decode"
        );
        let rows: [&[u8]; 3] = [&[0x0c], &[0x0c], &[0x00]];
        assert_eq!(
            t6::encode_g4(&rows, 8, 0)?,
            pass_stream,
            "the encoder must pass under a span"
        );
        Ok(())
    }

    /// Known answers from ITU-T T.4 Tables 2 and 3a: EOL W1728 W0 for a
    /// blank 1728-pixel row, and EOL W0 B8 for one black byte.
    #[test]
    fn g3_known_answers_decode() -> UnitResult {
        let blank = decode_strip(1728, 1, |strip, reference| {
            let stream = [0x00, 0x14, 0xd9, 0xa8, 0x00, 0x80];
            t4::decode_g3_strip(&stream, strip, reference, false, &BOUNDS)
        })?;
        assert_eq!(
            blank,
            vec![0_u8; 216],
            "W1728 W0 must decode to a blank row"
        );
        let black = decode_strip(8, 1, |strip, reference| {
            t4::decode_g3_strip(&[0x00, 0x13, 0x51, 0x40], strip, reference, false, &BOUNDS)
        })?;
        assert_eq!(black, vec![0xff], "W0 B8 must decode to one black byte");
        let rows: [&[u8]; 1] = [&[0_u8; 216]];
        let coded = t4::encode_g3(&rows, 1728, 0)?;
        assert_eq!(
            coded.get(..5),
            Some([0x00, 0x14, 0xd9, 0xa8, 0x00].as_slice()),
            "the encoder must emit W1728 W0"
        );
        Ok(())
    }

    /// A G3 strip under `T4Options` bit 0: a 1D row, then a 2D row, each
    /// after fill zeros and an EOL with its tag bit.
    #[test]
    fn g3_two_dimensional_rows_decode() -> UnitResult {
        let mut writer = BitWriter::new();
        writer.push_code((0, 5));
        for code in [(EOL_BITS, EOL_LEN), (1, 1), (0x07, 4), (0x03, 2), (0x0b, 4)] {
            writer.push_code(code);
        }
        writer.push_code((EOL_BITS, EOL_LEN));
        writer.push_code((0, 1));
        for code in [
            MODE_VERTICAL[1],
            MODE_VERTICAL[0],
            MODE_PASS,
            MODE_VERTICAL[3],
        ] {
            writer.push_code(code);
        }
        let stream = writer.finish();
        let bitmap = decode_strip(8, 2, |strip, reference| {
            t4::decode_g3_strip(&stream, strip, reference, true, &BOUNDS)
        })?;
        assert_eq!(bitmap, vec![0x30, 0x80], "the 1D then 2D rows must decode");
        Ok(())
    }

    #[test]
    fn a_corrupt_strip_names_its_page_and_row() {
        let refused = decode_strip(8, 2, |strip, reference| {
            t6::decode_g4_strip(&[0x00, 0x00, 0x00], strip, reference, &BOUNDS)
        });
        assert_eq!(
            refused,
            Err(TiffError::CorruptStrip { page: 0, row: 0 }),
            "zero bits are no mode code"
        );
    }

    /// Whether `left` is a prefix of `right` (or the same code).
    fn prefixes(left: (u16, u8), right: (u16, u8)) -> bool {
        left.1 <= right.1
            && right
                .0
                .checked_shr(u32::from(right.1.saturating_sub(left.1)))
                == Some(left.0)
    }

    /// No code of one colour, its make-ups, the shared extended make-ups or
    /// the EOL prefixes another.
    fn check_colour(name: &str, tables: &[&[(u16, u8, u16)]]) {
        let mut flat: Vec<(u16, u8)> = tables
            .iter()
            .flat_map(|table| table.iter().map(|entry| (entry.0, entry.1)))
            .collect();
        flat.push((EOL_BITS, EOL_LEN));
        for (left_index, &left) in flat.iter().enumerate() {
            for (right_index, &right) in flat.iter().enumerate() {
                assert!(
                    left_index == right_index || !prefixes(left, right),
                    "{name} code {left_index} prefixes code {right_index}"
                );
            }
        }
    }

    #[test]
    fn each_colour_code_set_is_prefix_free() {
        check_colour("white", &[&WHITE_TERM, &WHITE_MAKEUP, &EXTENDED_MAKEUP]);
        check_colour("black", &[&BLACK_TERM, &BLACK_MAKEUP, &EXTENDED_MAKEUP]);
        let modes: Vec<(u16, u8, u16)> = MODE_VERTICAL
            .iter()
            .map(|code| (code.0, code.1, 0))
            .collect();
        check_colour(
            "mode",
            &[
                &modes,
                &[
                    (MODE_PASS.0, MODE_PASS.1, 0),
                    (MODE_HORIZ.0, MODE_HORIZ.1, 0),
                ],
            ],
        );
    }

    /// A little-endian one-page TIFF built by hand: one IFD of `(tag, type,
    /// count, value)` entries in ascending tag order (StripOffsets pointed
    /// just past the IFD), then `tail`, the strip first.
    fn hand_tiff(entries: &[(u16, u16, u32, u32)], tail: &[u8]) -> Result<Vec<u8>, TiffError> {
        let data_at = 14_usize.saturating_add(entries.len().saturating_mul(12));
        let (Ok(data_at), Ok(count)) = (u32::try_from(data_at), u16::try_from(entries.len()))
        else {
            return crate::legacy::refuse(TiffError::BadHeader);
        };
        let mut out: Vec<u8> = vec![0x49, 0x49, 42, 0, 8, 0, 0, 0];
        out.extend_from_slice(&count.to_le_bytes());
        for &(tag, kind, total, value) in entries.iter() {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&total.to_le_bytes());
            let value = if tag == 273 { data_at } else { value };
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(tail);
        Ok(out)
    }

    /// TIFF 6.0 as real fax files carry it: a BlackIsZero page stored least
    /// significant bit first beside a RATIONAL XResolution the reader passes
    /// over reads as the black-is-set bitmap, padding cleared; uncompressed
    /// mode is a typed refusal.
    #[test]
    fn photometric_fill_order_and_foreign_tags_are_honoured() -> UnitResult {
        let mut entries = [
            (256, 3, 1, 4),
            (257, 3, 1, 1),
            (258, 3, 1, 1),
            (259, 3, 1, 1),
            (262, 3, 1, 1),
            (266, 3, 1, 2),
            (273, 4, 1, 0),
            (278, 3, 1, 1),
            (279, 4, 1, 1),
            (282, 5, 1, 135),
        ];
        let tail = [0xfa, 204, 0, 0, 0, 1, 0, 0, 0];
        let back = decode_tiff(&hand_tiff(&entries, &tail)?, &BOUNDS)?;
        assert_eq!(
            back,
            vec![TiffImage::from_bitmap(4, 1, &[0b1010_0000])?],
            "pixels 0 and 2 black after reversing and inverting"
        );
        entries[3] = (259, 3, 1, 3);
        entries[9] = (292, 4, 1, 2);
        assert!(
            matches!(
                decode_tiff(&hand_tiff(&entries, &tail)?, &BOUNDS),
                Err(TiffError::Unsupported { page: 0, .. })
            ),
            "T4Options uncompressed mode must be refused by name"
        );
        Ok(())
    }

    /// TIFF 6.0 Section 3 strips, each a G4 stream coded from a white row
    /// (ITU-T T.6 §2.2.1): one reference serves the page's two strips, and
    /// the second strip's first row must be read against white, not
    /// against the first strip's last row, which it was never coded from.
    #[test]
    fn each_strip_starts_from_a_white_reference_row() -> UnitResult {
        let rows: [[u8; 2]; 4] = [[0xff, 0x00], [0xff, 0x00], [0x0f, 0xf0], [0x0f, 0xf0]];
        let first = t6::encode_g4(&[&rows[0], &rows[1]], 16, 0)?;
        let second = t6::encode_g4(&[&rows[2], &rows[3]], 16, 0)?;
        // Eight IFD entries put the tail at 14 + 8 * 12 = 110: the offset
        // array there, the count array at 118, then the strips from 126.
        let (Ok(first_len), Ok(second_len)) =
            (u32::try_from(first.len()), u32::try_from(second.len()))
        else {
            return crate::legacy::refuse(TiffError::BadHeader);
        };
        let entries = [
            (256, 3, 1, 16),
            (257, 3, 1, 4),
            (258, 3, 1, 1),
            (259, 3, 1, 4),
            (262, 3, 1, 0),
            (273, 4, 2, 0),
            (278, 3, 1, 2),
            (279, 4, 2, 118),
        ];
        let mut tail: Vec<u8> = Vec::new();
        for word in [
            126,
            126_u32.saturating_add(first_len),
            first_len,
            second_len,
        ] {
            tail.extend_from_slice(&word.to_le_bytes());
        }
        tail.extend_from_slice(&first);
        tail.extend_from_slice(&second);
        let page: Vec<u8> = rows.iter().flatten().copied().collect();
        assert_eq!(
            decode_tiff(&hand_tiff(&entries, &tail)?, &BOUNDS)?,
            vec![TiffImage::from_bitmap(16, 4, &page)?],
            "both strips must decode, the second from a white row"
        );
        Ok(())
    }
}
