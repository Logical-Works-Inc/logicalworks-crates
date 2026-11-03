//! Seeded round-trip and hostile simulations of the database codec.
//!
//! One seed draws every table, a failure names the seed that produced it,
//! and the same seed must produce the same trace hash (Jet `STANDARD JET
//! DB` and ACE `STANDARD ACE DB` magics, page headers, the catalogue and
//! fixed rows; encrypted files and unsupported types are typed errors
//! naming the table and column).

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::access::{
    AccessDb, AccessError, CellValue, ColumnDef, ColumnTag, DbVersion, rows,
};
use lgwks_std::legacy::decimal::Decimal;
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, fold_bytes, next_byte};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, assert_verdict_replays,
    fold_usize, initial_trace,
};

/// The bounds every sweep here runs under.
const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

/// The column shapes the sweeps build.
const SHAPES: [(ColumnTag, u16); 5] = [
    (ColumnTag::Text, 8),
    (ColumnTag::Integer, 0),
    (ColumnTag::Currency, 0),
    (ColumnTag::Date, 0),
    (ColumnTag::Memo, 0),
];

/// A seeded cell of one shape.
fn draw_cell(state: &mut Seeded, tag: ColumnTag) -> CellValue {
    match tag {
        ColumnTag::Text => {
            let mut text = String::new();
            for _ in 0..below(state, 8) {
                let mut letter = b'A';
                for _ in 0..below(state, 26) {
                    letter = letter.wrapping_add(1);
                }
                text.push(char::from(letter));
            }
            CellValue::Text(text)
        }
        ColumnTag::Integer => {
            let mut magnitude: i32 = 0;
            for _ in 0..2 {
                magnitude = magnitude
                    .saturating_mul(256)
                    .saturating_add(i32::from(next_byte(state)));
            }
            if below(state, 2) == 0 {
                CellValue::Integer(magnitude)
            } else {
                CellValue::Integer(magnitude.saturating_neg())
            }
        }
        ColumnTag::Currency => {
            let mut magnitude: i128 = 0;
            for _ in 0..4 {
                magnitude = magnitude
                    .saturating_mul(256)
                    .saturating_add(i128::from(next_byte(state)));
            }
            CellValue::Currency(Decimal::new(magnitude, 4))
        }
        ColumnTag::Date => CellValue::Date {
            days: 20_000,
            millis: 0,
        },
        // Memo, and any shape a later release adds: the nullable text cell.
        _ => {
            if below(state, 2) == 0 {
                CellValue::Null
            } else {
                CellValue::Memo(String::from("memo"))
            }
        }
    }
}

/// Runs the seeded round-trip sweep and returns its trace.
fn access_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..8 {
        let shape = below(&mut state, SHAPES.len());
        let Some((tag, width)) = SHAPES.get(shape).copied() else {
            continue;
        };
        let defs = vec![ColumnDef::new("C0", tag, width)];
        let mut made: Vec<Vec<CellValue>> = Vec::new();
        for _ in 0..below(&mut state, 4) {
            made.push(vec![draw_cell(&mut state, tag)]);
        }
        let tables = vec![("T", defs.as_slice(), made.as_slice())];
        let got = rows::build_image(DbVersion::Jet4, &tables, &BOUNDS)
            .and_then(|image| AccessDb::open(&image, &BOUNDS))
            .and_then(|db| {
                Ok(db
                    .rows("T")?
                    .iter()
                    .map(|row| row.cells().to_vec())
                    .collect::<Vec<_>>())
            });
        assert_eq!(
            got.as_ref(),
            Ok(&made),
            "seed {seed}: every {tag:?} row must read back cell for cell"
        );
        fold_bytes(&mut trace, format!("{made:?}").as_bytes());
        fold_usize(&mut trace, made.len());
    }
    trace
}

#[test]
/// A seeded table opens and every row reads back in order.
fn a_seeded_table_round_trips() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let defs = vec![
            ColumnDef::new("NAME", ColumnTag::Text, 8),
            ColumnDef::new("AGE", ColumnTag::Integer, 0),
        ];
        let mut made: Vec<Vec<CellValue>> = Vec::new();
        for _ in 0..below(&mut state, 4).saturating_add(1) {
            made.push(vec![
                draw_cell(&mut state, ColumnTag::Text),
                draw_cell(&mut state, ColumnTag::Integer),
            ]);
        }
        let tables = vec![("PEOPLE", defs.as_slice(), made.as_slice())];
        let Ok(image) = rows::build_image(DbVersion::Ace, &tables, &BOUNDS) else {
            continue;
        };
        let Ok(db) = AccessDb::open(&image, &BOUNDS) else {
            continue;
        };
        assert_eq!(
            db.tables(),
            vec![String::from("PEOPLE")],
            "seed {seed}: the catalogue must list the table"
        );
        assert_eq!(
            db.version(),
            DbVersion::Ace,
            "seed {seed}: the engine must read back"
        );
        let Ok(got) = db.rows("PEOPLE") else {
            continue;
        };
        assert_eq!(
            got.len(),
            made.len(),
            "seed {seed}: every row must read back"
        );
    }
}

#[test]
/// Hostile images never panic: flips and cuts answer typed errors, and the
/// encrypted flag refuses the file.
fn hostile_images_answer_typed_errors() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let defs = vec![ColumnDef::new("A", ColumnTag::Integer, 0)];
        let made = vec![vec![CellValue::Integer(1)]];
        let tables = vec![("T", defs.as_slice(), made.as_slice())];
        let Ok(image) = rows::build_image(DbVersion::Jet3, &tables, &BOUNDS) else {
            continue;
        };
        let mut hostile = image.clone();
        let position = below(&mut state, hostile.len().max(1));
        if let Some(slot) = hostile.get_mut(position) {
            *slot = slot.wrapping_add(1);
        }
        assert_verdict_replays(seed, "flipped database", || {
            AccessDb::open(&hostile, &BOUNDS)
        });
        let cut = below(&mut state, image.len());
        if let Some(prefix) = image.get(..cut) {
            assert!(
                AccessDb::open(prefix, &BOUNDS).is_err(),
                "seed {seed}: a database cut to {cut} of {} bytes must be refused",
                image.len()
            );
        }
        let mut flagged = hostile.clone();
        if let Some(slot) = flagged.get_mut(22) {
            *slot = 0x01;
        }
        assert!(
            AccessDb::open(&flagged, &BOUNDS).is_err(),
            "seed {seed}: the encrypted flag must refuse"
        );
    }
}

/// Bytes of one Jet 4 page.
const JET4_PAGE: usize = 4096;

#[test]
/// A table larger than one page spills whole rows onto further pages and
/// reads back in order; past the 255 pages a catalogue entry lists, the
/// writer refuses by table instead of truncating.
fn a_table_past_one_page_spans_pages_and_a_table_past_255_is_refused() {
    let wide = Limits::new(16_777_216, 100_000, 4096, 8, 16_777_216);
    let defs = vec![
        ColumnDef::new("NAME", ColumnTag::Text, 200),
        ColumnDef::new("N", ColumnTag::Integer, 0),
    ];
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        let count = below(&mut state, 400).saturating_add(100);
        let mut made: Vec<Vec<CellValue>> = Vec::with_capacity(count);
        for index in 0..count {
            let Ok(number) = i32::try_from(index) else {
                continue;
            };
            made.push(vec![
                draw_cell(&mut state, ColumnTag::Text),
                CellValue::Integer(number),
            ]);
        }
        let tables = vec![("BIG", defs.as_slice(), made.as_slice())];
        let image = rows::build_image(DbVersion::Jet4, &tables, &wide);
        let pages = match image {
            Ok(ref bytes) => bytes.len().div_euclid(JET4_PAGE),
            Err(_) => 0,
        };
        assert!(
            pages > 4,
            "seed {seed}: {count} rows of 204 bytes must fill several data pages, filled {pages}"
        );
        let got = image
            .and_then(|bytes| AccessDb::open(&bytes, &wide))
            .and_then(|db| {
                Ok(db
                    .rows("BIG")?
                    .iter()
                    .map(|row| row.cells().to_vec())
                    .collect::<Vec<_>>())
            });
        assert_eq!(
            got.as_ref(),
            Ok(&made),
            "seed {seed}: every row must read back across pages in order"
        );
    }
    let too_many: Vec<Vec<CellValue>> = (0..6_000)
        .map(|index| {
            vec![
                CellValue::Text(String::from("X")),
                CellValue::Integer(index),
            ]
        })
        .collect();
    let tables = vec![("HUGE", defs.as_slice(), too_many.as_slice())];
    assert!(
        matches!(
            rows::build_image(DbVersion::Jet4, &tables, &wide),
            Err(AccessError::ValueMismatch { ref table, .. }) if table == "HUGE"
        ),
        "a table past 255 data pages must be refused by name"
    );
}

#[test]
/// The replay oracle: one seed, one trace.
fn the_same_seed_replays_to_the_same_access_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(access_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_access_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(access_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
