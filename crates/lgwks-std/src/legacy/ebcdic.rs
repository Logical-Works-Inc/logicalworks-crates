//! `ebcdic` decodes and encodes the EBCDIC code pages COBOL files arrive in.
//!
//! Code pages 037 (US/Canada), 500 (International), 1047 (Latin-1 Open
//! Systems) and 273 (Austria/Germany) are each a complete permutation of
//! ISO-8859-1: every one of the 256 bytes decodes to one character in
//! U+0000-U+00FF and every such character encodes to one byte, per the IBM
//! Character Data Representation Architecture tables (SC09-2190). So decode
//! then encode returns the same bytes for every input, and only a character
//! past U+00FF is refused, as [`EbcdicError::UnmappableChar`] naming its
//! offset.
//!
//! The tables are stored the way IBM defines the pages: 037 whole, and each
//! other page as the national-use positions where it differs from 037 (500 at
//! seven, 1047 at six, 273 at twenty-one). Each page's forward and reverse
//! tables are built once, so a decode or an encode is one index per byte.

use std::fmt;
use std::sync::LazyLock;

use super::limits::{LimitError, Limits};

/// The EBCDIC code page a decode or encode runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodePage {
    /// US/Canada, CPGID 037.
    Cp037,
    /// International, CPGID 500.
    Cp500,
    /// Latin-1 Open Systems, CPGID 1047.
    Cp1047,
    /// Austria/Germany, CPGID 273.
    Cp273,
}

/// Code page 037: the ISO-8859-1 code point each byte decodes to, sixteen
/// bytes per row in byte order (CDRA CPGID 037, CCSID 37).
const BASE_037: [u8; 256] = [
    0x00, 0x01, 0x02, 0x03, 0x9c, 0x09, 0x86, 0x7f, 0x97, 0x8d, 0x8e, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x9d, 0x85, 0x08, 0x87, 0x18, 0x19, 0x92, 0x8f, 0x1c, 0x1d, 0x1e, 0x1f,
    0x80, 0x81, 0x82, 0x83, 0x84, 0x0a, 0x17, 0x1b, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x05, 0x06, 0x07,
    0x90, 0x91, 0x16, 0x93, 0x94, 0x95, 0x96, 0x04, 0x98, 0x99, 0x9a, 0x9b, 0x14, 0x15, 0x9e, 0x1a,
    0x20, 0xa0, 0xe2, 0xe4, 0xe0, 0xe1, 0xe3, 0xe5, 0xe7, 0xf1, 0xa2, 0x2e, 0x3c, 0x28, 0x2b, 0x7c,
    0x26, 0xe9, 0xea, 0xeb, 0xe8, 0xed, 0xee, 0xef, 0xec, 0xdf, 0x21, 0x24, 0x2a, 0x29, 0x3b, 0xac,
    0x2d, 0x2f, 0xc2, 0xc4, 0xc0, 0xc1, 0xc3, 0xc5, 0xc7, 0xd1, 0xa6, 0x2c, 0x25, 0x5f, 0x3e, 0x3f,
    0xf8, 0xc9, 0xca, 0xcb, 0xc8, 0xcd, 0xce, 0xcf, 0xcc, 0x60, 0x3a, 0x23, 0x40, 0x27, 0x3d, 0x22,
    0xd8, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0xab, 0xbb, 0xf0, 0xfd, 0xfe, 0xb1,
    0xb0, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0x71, 0x72, 0xaa, 0xba, 0xe6, 0xb8, 0xc6, 0xa4,
    0xb5, 0x7e, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0xa1, 0xbf, 0xd0, 0xdd, 0xde, 0xae,
    0x5e, 0xa3, 0xa5, 0xb7, 0xa9, 0xa7, 0xb6, 0xbc, 0xbd, 0xbe, 0x5b, 0x5d, 0xaf, 0xa8, 0xb4, 0xd7,
    0x7b, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0xad, 0xf4, 0xf6, 0xf2, 0xf3, 0xf5,
    0x7d, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52, 0xb9, 0xfb, 0xfc, 0xf9, 0xfa, 0xff,
    0x5c, 0xf7, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0xb2, 0xd4, 0xd6, 0xd2, 0xd3, 0xd5,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0xb3, 0xdb, 0xdc, 0xd9, 0xda, 0x9f,
];

/// Code page 500 as `(byte, code point)` where it differs from 037: the
/// International brackets, exclamation, circumflex, cent, not and bar.
const DELTA_500: [(u8, u8); 7] = [
    (0x4a, b'['),
    (0x4f, b'!'),
    (0x5a, b']'),
    (0x5f, b'^'),
    (0xb0, 0xa2),
    (0xba, 0xac),
    (0xbb, b'|'),
];

/// Code page 1047 where it differs from 037: brackets at 0xAD and 0xBD, the
/// circumflex at 0x5F, and the not sign, Y-acute and diaeresis they displace.
const DELTA_1047: [(u8, u8); 6] = [
    (0x5f, b'^'),
    (0xad, b'['),
    (0xb0, 0xac),
    (0xba, 0xdd),
    (0xbb, 0xa8),
    (0xbd, b']'),
];

/// Code page 273 where it differs from 037: the umlauts and eszett on the
/// national-use bytes, and the brackets, braces and symbols they displace.
const DELTA_273: [(u8, u8); 21] = [
    (0x43, b'{'),
    (0x4a, 0xc4),
    (0x4f, b'!'),
    (0x59, b'~'),
    (0x5a, 0xdc),
    (0x5f, b'^'),
    (0x63, b'['),
    (0x6a, 0xf6),
    (0x7c, 0xa7),
    (0xa1, 0xdf),
    (0xb0, 0xa2),
    (0xb5, b'@'),
    (0xba, 0xac),
    (0xbb, b'|'),
    (0xc0, 0xe4),
    (0xcc, 0xa6),
    (0xd0, 0xfc),
    (0xdc, b'}'),
    (0xe0, 0xd6),
    (0xec, b'\\'),
    (0xfc, b']'),
];

/// One page's two tables: byte to code point, and code point to byte.
struct Page {
    /// The ISO-8859-1 code point each byte decodes to.
    forward: [u8; 256],
    /// The byte each ISO-8859-1 code point encodes to.
    reverse: [u8; 256],
}

impl Page {
    /// Builds a page from 037 and its differences, then inverts it.
    fn from_deltas(deltas: &[(u8, u8)]) -> Self {
        let mut forward = BASE_037;
        for &(byte, point) in deltas {
            if let Some(slot) = forward.get_mut(usize::from(byte)) {
                *slot = point;
            }
        }
        let mut reverse = [0_u8; 256];
        for (byte, &point) in (0..=u8::MAX).zip(forward.iter()) {
            if let Some(slot) = reverse.get_mut(usize::from(point)) {
                *slot = byte;
            }
        }
        Self { forward, reverse }
    }
}

/// Code page 037, built on first use.
static PAGE_037: LazyLock<Page> = LazyLock::new(|| Page::from_deltas(&[]));

/// Code page 500, built on first use.
static PAGE_500: LazyLock<Page> = LazyLock::new(|| Page::from_deltas(&DELTA_500));

/// Code page 1047, built on first use.
static PAGE_1047: LazyLock<Page> = LazyLock::new(|| Page::from_deltas(&DELTA_1047));

/// Code page 273, built on first use.
static PAGE_273: LazyLock<Page> = LazyLock::new(|| Page::from_deltas(&DELTA_273));

/// The tables a page decodes and encodes through.
fn page_of(page: CodePage) -> &'static Page {
    match page {
        CodePage::Cp037 => &PAGE_037,
        CodePage::Cp500 => &PAGE_500,
        CodePage::Cp1047 => &PAGE_1047,
        CodePage::Cp273 => &PAGE_273,
    }
}

/// Why an EBCDIC decode or encode failed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EbcdicError {
    /// A byte the page maps to nothing, at its input offset.
    UnmappableByte {
        /// Zero-based offset of the offending byte.
        offset: usize,
        /// The offending byte, verbatim.
        byte: u8,
    },
    /// A character past U+00FF, which no EBCDIC page here encodes.
    UnmappableChar {
        /// Zero-based byte offset of the offending character in the input text.
        offset: usize,
        /// The offending character, verbatim.
        value: char,
    },
    /// The input or output passed a [`Limits`] ceiling before any mapping ran.
    Limit(LimitError),
}

impl fmt::Display for EbcdicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnmappableByte { offset, byte } => write!(
                formatter,
                "EBCDIC byte {byte:#04x} at offset {offset} maps to no character"
            ),
            Self::UnmappableChar { offset, value } => write!(
                formatter,
                "character {value:?} at offset {offset} maps to no EBCDIC byte"
            ),
            Self::Limit(ref cause) => write!(formatter, "EBCDIC limits refused the input: {cause}"),
        }
    }
}

crate::legacy::limits::limit_carrying_error!(EbcdicError);

/// Decodes EBCDIC bytes into text under a code page.
///
/// # Errors
///
/// [`EbcdicError::Limit`] when the input passes the byte ceiling. Every byte
/// maps on every page, so nothing else refuses.
pub fn decode(page: CodePage, bytes: &[u8], limits: &Limits) -> Result<String, EbcdicError> {
    limits.check_bytes(bytes.len())?;
    let table = &page_of(page).forward;
    let mut out = String::with_capacity(bytes.len());
    for (offset, &byte) in bytes.iter().enumerate() {
        let Some(&point) = table.get(usize::from(byte)) else {
            return crate::legacy::refuse(EbcdicError::UnmappableByte { offset, byte });
        };
        out.push(char::from(point));
    }
    Ok(out)
}

/// Encodes text into EBCDIC bytes under a code page.
///
/// # Errors
///
/// [`EbcdicError::Limit`] when the text passes the byte ceiling,
/// [`EbcdicError::UnmappableChar`] naming the byte offset of the first
/// character past U+00FF.
pub fn encode(page: CodePage, text: &str, limits: &Limits) -> Result<Vec<u8>, EbcdicError> {
    limits.check_bytes(text.len())?;
    let table = &page_of(page).reverse;
    let mut out = Vec::with_capacity(text.len());
    for (offset, value) in text.char_indices() {
        let Some(&byte) = u8::try_from(value)
            .ok()
            .and_then(|point| table.get(usize::from(point)))
        else {
            let refusal = Err(EbcdicError::UnmappableChar { offset, value });
            #[cfg(feature = "trace")]
            crate::trace::debug!(offset, "ebcdic encode: refusing a character past U+00FF");
            return refusal;
        };
        out.push(byte);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{CodePage, EbcdicError, decode, encode};
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under: generous, so only hostile sizes trip.
    const BOUNDS: Limits = Limits::new(1024, 1024, 64, 8, 4096);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), EbcdicError>;

    /// Every page this module carries.
    const PAGES: [CodePage; 4] = [
        CodePage::Cp037,
        CodePage::Cp500,
        CodePage::Cp1047,
        CodePage::Cp273,
    ];

    /// Each page is a permutation of ISO-8859-1: all 256 bytes decode to 256
    /// distinct characters, and they encode back to the same bytes.
    #[test]
    fn every_page_is_a_bijection_onto_latin_1() -> UnitResult {
        let every: Vec<u8> = (0..=u8::MAX).collect();
        for page in PAGES {
            let text = decode(page, &every, &BOUNDS)?;
            let mut seen: Vec<char> = text.chars().collect();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(
                seen.len(),
                256,
                "{page:?} must decode 256 distinct characters"
            );
            assert!(
                seen.iter().all(|&value| u32::from(value) <= 0xff),
                "{page:?} must decode into U+0000-U+00FF"
            );
            assert_eq!(
                encode(page, &text, &BOUNDS)?,
                every,
                "{page:?} must encode every byte back"
            );
        }
        Ok(())
    }

    /// Known answers from the IBM CDRA tables: one string, four spellings.
    #[test]
    fn the_pages_spell_known_text_as_ibm_defines_it() -> UnitResult {
        let hello: [u8; 13] = [
            0xc8, 0x85, 0x93, 0x93, 0x96, 0x6b, 0x40, 0xe6, 0x96, 0x99, 0x93, 0x84, 0x5a,
        ];
        assert_eq!(
            encode(CodePage::Cp037, "Hello, World!", &BOUNDS)?,
            hello,
            "037 spells ! as 0x5A"
        );
        assert_eq!(
            encode(CodePage::Cp500, "[!]^", &BOUNDS)?,
            vec![0x4a, 0x4f, 0x5a, 0x5f],
            "500 holds brackets, exclamation and circumflex on the national bytes"
        );
        assert_eq!(
            encode(CodePage::Cp1047, "[]^¬", &BOUNDS)?,
            vec![0xad, 0xbd, 0x5f, 0xb0],
            "1047 holds brackets at 0xAD and 0xBD"
        );
        assert_eq!(
            decode(
                CodePage::Cp273,
                &[0x4a, 0x5a, 0x6a, 0xa1, 0xc0, 0xd0, 0xe0, 0x7c, 0xb5],
                &BOUNDS
            )?,
            String::from("ÄÜößäüÖ§@"),
            "273 holds the umlauts and eszett on the national bytes"
        );
        assert_eq!(
            decode(CodePage::Cp037, &[0x4a, 0x25, 0x15], &BOUNDS)?,
            String::from("¢\n\u{85}"),
            "037 reads cent, LF and NEL"
        );
        Ok(())
    }

    #[test]
    fn a_character_past_latin_1_names_its_offset() {
        assert_eq!(
            encode(CodePage::Cp037, "ok€", &BOUNDS),
            Err(EbcdicError::UnmappableChar {
                offset: 2,
                value: '€'
            }),
            "the euro sign is past U+00FF on 037 and must be refused at its offset"
        );
    }

    #[test]
    fn a_zero_ceiling_refuses_both_directions() {
        let tight = Limits::new(0, 0, 0, 0, 0);
        assert!(
            decode(CodePage::Cp037, &[0x40], &tight).is_err(),
            "a zero-byte ceiling must refuse a decode"
        );
        assert!(
            encode(CodePage::Cp037, " ", &tight).is_err(),
            "a zero-byte ceiling must refuse an encode"
        );
    }
}
