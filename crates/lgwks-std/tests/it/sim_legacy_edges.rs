//! Seeded edge simulations of the legacy codecs: the cases #792 fixed.
//!
//! Each test draws from one seed and names it on failure: every byte of every
//! EBCDIC page round-trips (IBM CDRA CPGID 037/500/1047/273 are permutations
//! of ISO-8859-1), copybook text follows the layout's page and a zero-count
//! `OCCURS DEPENDING ON` table round-trips (Enterprise COBOL Language
//! Reference), MS-CFB §2.6.1 sibling names compare uppercased, fax bitmaps
//! clear the bits past the width (ITU-T T.4/T.6 rows are `width` pixels),
//! and Access tables never lose a row to a full page.

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::access::{AccessDb, CellValue, ColumnDef, ColumnTag, DbVersion, rows};
use lgwks_std::legacy::cfb::{CfbError, CfbFile, build_image};
use lgwks_std::legacy::copybook::{CopybookError, FieldValue, RecordLayout};
use lgwks_std::legacy::decimal::{Decimal, decode_packed, encode_packed};
use lgwks_std::legacy::ebcdic::{CodePage, EbcdicError, decode, encode};
use lgwks_std::legacy::fax::{Compression, TiffError, TiffImage, decode_tiff, encode_tiff};
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, fold_bytes, next_byte, next_bytes};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, initial_trace,
};

/// The bounds every simulation here runs under.
const BOUNDS: Limits = Limits::new(4_194_304, 65_536, 1024, 8, 4_194_304);

/// Every page the codec carries.
const PAGES: [CodePage; 4] = [
    CodePage::Cp037,
    CodePage::Cp500,
    CodePage::Cp1047,
    CodePage::Cp273,
];

/// The bits a one-byte row keeps, by its width in pixels minus one.
const KEEP: [u8; 8] = [0x80, 0xc0, 0xe0, 0xf0, 0xf8, 0xfc, 0xfe, 0xff];

/// A seeded coefficient of up to seven decimal digits.
fn draw_seven_digits(state: &mut Seeded) -> i128 {
    let mut value: i128 = 0;
    for _ in 0..7 {
        value = value
            .saturating_mul(10)
            .saturating_add(i128::from(next_byte(state).rem_euclid(10)));
    }
    value
}

#[test]
/// Any seeded byte string decodes and re-encodes to itself on every page.
fn sim_every_page_round_trips_a_seeded_byte_string() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let len = below(&mut state, 512);
        let bytes = next_bytes(&mut state, len);
        for page in PAGES {
            let back = decode(page, &bytes, &BOUNDS).and_then(|text| encode(page, &text, &BOUNDS));
            assert_eq!(
                back,
                Ok(bytes.clone()),
                "seed {seed}: {page:?} must return {len} drawn bytes"
            );
        }
    }
}

#[test]
/// Letters and digits sit on the invariant bytes: every page reads them alike.
fn sim_invariant_letters_and_digits_read_alike_on_every_page() {
    const INVARIANT: [u8; 19] = [
        0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5,
        0xf6, 0xf7, 0xf8, 0xf9,
    ];
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let mut bytes = Vec::with_capacity(32);
        for _ in 0..32 {
            if let Some(&byte) = INVARIANT.get(below(&mut state, INVARIANT.len())) {
                bytes.push(byte);
            }
        }
        let read: Vec<Result<String, EbcdicError>> = PAGES
            .iter()
            .map(|&page| decode(page, &bytes, &BOUNDS))
            .collect();
        assert!(
            read.windows(2)
                .all(|pair| matches!(pair, [first, second] if first == second)),
            "seed {seed}: invariant bytes must read alike on every page: {read:?}"
        );
    }
}

#[test]
/// A character past U+00FF after a seeded Latin-1 prefix is refused at its byte offset.
fn sim_a_character_past_latin_1_is_refused_at_its_offset() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let prefix_len = below(&mut state, 40);
        let prefix: String = (0..prefix_len)
            .map(|_| char::from(next_byte(&mut state).rem_euclid(26).saturating_add(b'a')))
            .collect();
        let text = format!("{prefix}\u{20ac}tail");
        for page in PAGES {
            assert_eq!(
                encode(page, &text, &BOUNDS),
                Err(EbcdicError::UnmappableChar {
                    offset: prefix_len,
                    value: '\u{20ac}'
                }),
                "seed {seed}: {page:?} must name offset {prefix_len}"
            );
        }
    }
}

#[test]
/// A seeded signed value packs into seven digits and unpacks to itself.
fn sim_packed_decimal_keeps_a_seeded_value_and_its_sign() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..64 {
            let magnitude = draw_seven_digits(&mut state);
            let value = if below(&mut state, 2) == 0 {
                magnitude
            } else {
                magnitude.saturating_neg()
            };
            let back = encode_packed(Decimal::new(value, 2), 7, true, &BOUNDS)
                .and_then(|bytes| decode_packed(&bytes, 7, 2, true, &BOUNDS));
            assert_eq!(
                back,
                Ok(Decimal::new(value, 2)),
                "seed {seed}: {value} must survive COMP-3"
            );
        }
    }
}

#[test]
/// Copybook text on page 500 is the page-500 spelling and decodes back.
fn sim_copybook_text_follows_its_code_page() -> Result<(), CopybookError> {
    const ALPHABET: &[u8] = b"[]!^AB09 ";
    let layout =
        RecordLayout::parse("01 R.\n05 F PIC X(6).\n", &BOUNDS)?.with_code_page(CodePage::Cp500);
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let mut text = String::with_capacity(6);
        for _ in 0..6 {
            if let Some(&byte) = ALPHABET.get(below(&mut state, ALPHABET.len())) {
                text.push(char::from(byte));
            }
        }
        let values = vec![FieldValue::Text(text.clone())];
        let record = layout.encode(&values, &BOUNDS)?;
        assert_eq!(
            Ok(record.clone()),
            encode(CodePage::Cp500, &text, &BOUNDS),
            "seed {seed}: {text:?} on page 500"
        );
        assert_eq!(
            layout.decode(&record, &BOUNDS)?,
            values,
            "seed {seed}: {text:?} must decode back"
        );
    }
    Ok(())
}

#[test]
/// `OCCURS 0 TO 3 DEPENDING ON` round-trips every seeded count, zero included.
fn sim_a_depending_table_round_trips_every_count_from_zero() -> Result<(), CopybookError> {
    const TEXT: &str = "01 R.\n05 N PIC 9(1).\n05 T OCCURS 0 TO 3 TIMES DEPENDING ON N PIC X(2).\n";
    let layout = RecordLayout::parse(TEXT, &BOUNDS)?;
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..8 {
            let count = below(&mut state, 4);
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let pair: String = (0..2)
                    .map(|_| char::from(next_byte(&mut state).rem_euclid(26).saturating_add(b'A')))
                    .collect();
                items.push(FieldValue::Text(pair));
            }
            let Ok(held) = i128::try_from(count) else {
                continue;
            };
            let values = vec![
                FieldValue::Number(Decimal::new(held, 0)),
                FieldValue::Group(items),
            ];
            let record = layout.encode(&values, &BOUNDS)?;
            assert_eq!(
                layout.decode(&record, &BOUNDS)?,
                values,
                "seed {seed}: {count} dependents must round-trip"
            );
        }
    }
    Ok(())
}

#[test]
/// Seeded stream counts and contents all read back from one compound image.
fn sim_every_stream_of_a_seeded_image_reads_back() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let count = below(&mut state, 5).saturating_add(1);
        let names: Vec<String> = (0..count).map(|index| format!("Stream{index}")).collect();
        let mut datas = Vec::with_capacity(count);
        for _ in 0..count {
            let len = below(&mut state, 3000);
            datas.push(next_bytes(&mut state, len));
        }
        let streams: Vec<(&str, &[u8])> = names
            .iter()
            .zip(datas.iter())
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        let opened =
            build_image(&streams, &BOUNDS).and_then(|image| CfbFile::open(&image, &BOUNDS));
        let read: Result<Vec<Vec<u8>>, CfbError> = opened.and_then(|file| {
            names
                .iter()
                .map(|name| file.read_stream(name, &BOUNDS))
                .collect()
        });
        assert_eq!(
            read,
            Ok(datas),
            "seed {seed}: all {count} streams must read back"
        );
    }
}

#[test]
/// Two sibling names equal once uppercased are refused, naming the second.
fn sim_case_folded_sibling_names_are_refused() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let len = below(&mut state, 12).saturating_add(1);
        let lower: String = (0..len)
            .map(|_| char::from(next_byte(&mut state).rem_euclid(26).saturating_add(b'a')))
            .collect();
        let upper = lower.to_uppercase();
        let streams = [
            (lower.as_str(), b"one".as_slice()),
            (upper.as_str(), b"two".as_slice()),
        ];
        assert!(
            matches!(build_image(&streams, &BOUNDS), Err(CfbError::BadName { ref name, .. }) if *name == upper),
            "seed {seed}: {upper:?} must be refused beside {lower:?}"
        );
    }
}

#[test]
/// Seeded integer rows across many pages read back in order.
fn sim_integer_rows_across_pages_read_back_in_order() {
    let defs = vec![ColumnDef::new("V", ColumnTag::Integer, 0)];
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let count = below(&mut state, 3000).saturating_add(1500);
        let mut made = Vec::with_capacity(count);
        for _ in 0..count {
            let drawn = i32::from_be_bytes([
                next_byte(&mut state),
                next_byte(&mut state),
                next_byte(&mut state),
                next_byte(&mut state),
            ]);
            made.push(vec![CellValue::Integer(drawn)]);
        }
        let tables = vec![("NUMS", defs.as_slice(), made.as_slice())];
        let got = rows::build_image(DbVersion::Jet4, &tables, &BOUNDS)
            .and_then(|image| AccessDb::open(&image, &BOUNDS))
            .and_then(|db| {
                Ok(db
                    .rows("NUMS")?
                    .iter()
                    .map(|row| row.cells().to_vec())
                    .collect::<Vec<_>>())
            });
        assert_eq!(
            got,
            Ok(made),
            "seed {seed}: {count} integer rows must read back in order"
        );
    }
}

#[test]
/// A seeded odd-width page keeps every in-width bit through G4.
fn sim_an_odd_width_page_keeps_its_bits_through_g4() -> Result<(), TiffError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let width = below(&mut state, 60).saturating_mul(2).saturating_add(1);
        let height = below(&mut state, 12).saturating_add(1);
        let bitmap = next_bytes(&mut state, width.div_ceil(8).saturating_mul(height));
        let (Ok(width_narrow), Ok(height_narrow)) = (u32::try_from(width), u32::try_from(height))
        else {
            continue;
        };
        let page = TiffImage::from_bitmap(width_narrow, height_narrow, &bitmap)?;
        let coded = encode_tiff(core::slice::from_ref(&page), Compression::G4, &BOUNDS)?;
        assert_eq!(
            decode_tiff(&coded, &BOUNDS)?,
            vec![page],
            "seed {seed}: a {width}x{height} page must survive G4"
        );
    }
    Ok(())
}

#[test]
/// Bits past the width are cleared when an image is built, whatever was drawn there.
fn sim_padding_bits_are_cleared_by_from_bitmap() -> Result<(), TiffError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let width = below(&mut state, 8).saturating_add(1);
        let row = next_byte(&mut state);
        let (Ok(width_narrow), Some(&keep)) =
            (u32::try_from(width), KEEP.get(width.saturating_sub(1)))
        else {
            continue;
        };
        let page = TiffImage::from_bitmap(width_narrow, 1, &[row])?;
        assert_eq!(
            page.bitmap(),
            [row & keep].as_slice(),
            "seed {seed}: {width} pixels keep only their bits"
        );
    }
    Ok(())
}

/// One seeded pass over the EBCDIC and packed codecs, folded into a trace.
fn edges_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    let len = below(&mut state, 256);
    let bytes = next_bytes(&mut state, len);
    for page in PAGES {
        if let Ok(text) = decode(page, &bytes, &BOUNDS) {
            fold_bytes(&mut trace, text.as_bytes());
        }
    }
    if let Ok(packed) = encode_packed(
        Decimal::new(draw_seven_digits(&mut state), 0),
        7,
        true,
        &BOUNDS,
    ) {
        fold_bytes(&mut trace, &packed);
    }
    trace
}

#[test]
/// The replay oracle: one seed, one trace.
fn sim_the_same_seed_replays_to_the_same_edges_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(edges_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn sim_distinct_seeds_diverge_in_the_edges_trace() {
    assert_distinct_seeds_diverge(edges_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
