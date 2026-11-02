//! Seeded round-trip and hostile simulations of the copybook codec.
//!
//! One seed draws every record, a failure names the seed that produced it,
//! and the same seed must produce the same trace hash (IBM Enterprise COBOL
//! Language Reference, record description entries: levels 01-49, 66
//! RENAMES refused, 88 kept as names, `PIC X/9/S9/V/COMP/COMP-3/COMP-4` with
//! `OCCURS` and `REDEFINES`; dates keep their raw bytes beside the parsed
//! value).

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::copybook::{CopybookError, FieldValue, RecordLayout};
use lgwks_std::legacy::decimal::Decimal;
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, fold_bytes, next_byte};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, initial_trace,
};

/// The bounds every sweep here runs under.
const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

/// The customer copybook the sweeps parse.
const CUSTOMER: &str = "01 CUSTOMER.\n05 CUST-ID PIC 9(6).\n05 CUST-NAME PIC X(10).\n05 CUST-BAL PIC S9(5)V99 COMP-3.\n05 CUST-BORN-DATE PIC 9(8).\n05 DEP-COUNT PIC 9(2).\n05 DEP-NAME OCCURS 0 TO 3 TIMES DEPENDING ON DEP-COUNT PIC X(8).\n";

/// A seeded six-digit id for the `PIC 9(6)` zoned field.
fn draw_id(state: &mut Seeded) -> FieldValue {
    let mut id: i128 = 0;
    for _ in 0..6 {
        id = id
            .saturating_mul(10)
            .saturating_add(i128::from(next_byte(state).rem_euclid(10)));
    }
    FieldValue::Number(Decimal::new(id, 0))
}

/// A seeded name of `len` capitals, space-padded.
fn draw_name(state: &mut Seeded, len: usize) -> String {
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let mut letter = b'A';
        for _ in 0..below(state, 26) {
            letter = letter.wrapping_add(1);
        }
        out.push(char::from(letter));
    }
    while out.len() < len {
        out.push(' ');
    }
    out
}

/// Runs the seeded round-trip sweep and returns its trace.
fn copybook_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    let layout = match RecordLayout::parse(CUSTOMER, &BOUNDS) {
        Ok(drawn) => drawn,
        Err(_) => return trace,
    };
    for _ in 0..24 {
        let count = below(&mut state, 4);
        let mut members: Vec<FieldValue> = Vec::with_capacity(count);
        for _ in 0..count {
            members.push(FieldValue::Text(draw_name(&mut state, 8)));
        }
        let coefficient = i128::from(next_byte(&mut state)).saturating_mul(100);
        let held: i128 = match count {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 3,
        };
        let values = vec![
            draw_id(&mut state),
            FieldValue::Text(draw_name(&mut state, 10)),
            FieldValue::Number(Decimal::new(coefficient, 2)),
            FieldValue::Number(Decimal::new(19_900_131, 0)),
            FieldValue::Number(Decimal::new(held, 0)),
            FieldValue::Group(members),
        ];
        let record = layout.encode(&values, &BOUNDS);
        let again = record
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|bytes| layout.decode(bytes, &BOUNDS))
            .and_then(|restored| layout.encode(&restored, &BOUNDS));
        assert_eq!(
            again, record,
            "seed {seed}: {count} dependents must encode, and decode then encode must return the same bytes"
        );
        if let Ok(bytes) = record {
            fold_bytes(&mut trace, &bytes);
        }
    }
    trace
}

#[test]
/// A seeded record survives `encode` then `decode` with its bytes intact.
fn a_seeded_record_round_trips() -> Result<(), CopybookError> {
    let layout = RecordLayout::parse(CUSTOMER, &BOUNDS)?;
    assert_eq!(
        layout.record_len(),
        6_usize
            .saturating_add(10)
            .saturating_add(4)
            .saturating_add(8)
            .saturating_add(2)
            .saturating_add(24),
        "the layout must sum its fields"
    );
    let fields = layout.fields();
    assert_eq!(fields.len(), 6, "six declared fields must lay out");
    let Some(first) = fields.first() else {
        return Err(CopybookError::LengthMismatch { need: 1, have: 0 });
    };
    assert_eq!(first.name(), "CUST-ID", "the first field must be the id");
    assert_eq!(first.offset(), 0, "the first field starts at zero");
    assert_eq!(first.len(), 6, "the id holds six zoned bytes");
    assert_eq!(first.kind(), "9", "the id is unsigned zoned");
    assert_eq!(first.scale(), 0, "the id has no scale");
    assert!(!first.is_dynamic(), "the id is a fixed field");
    assert_eq!(first.max_occurs(), 1, "the id occurs once");
    assert!(first.children().is_empty(), "a leaf has no children");
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let values = vec![
                draw_id(&mut state),
                FieldValue::Text(draw_name(&mut state, 10)),
                FieldValue::Number(Decimal::new(10_000, 2)),
                FieldValue::Number(Decimal::new(20_000_229, 0)),
                FieldValue::Number(Decimal::new(1, 0)),
                FieldValue::Group(vec![FieldValue::Text(draw_name(&mut state, 8))]),
            ];
            let record = layout.encode(&values, &BOUNDS)?;
            let back = layout.decode(&record, &BOUNDS)?;
            assert_eq!(back.len(), 6, "six values must decode");
            let Some(date_value) = back.get(3) else {
                return Err(CopybookError::LengthMismatch { need: 4, have: 0 });
            };
            match *date_value {
                FieldValue::Date(ref dated) => {
                    assert_eq!(dated.year(), 2000, "the date year must parse");
                    assert_eq!(dated.month(), 2, "the date month must parse");
                    assert_eq!(dated.day(), 29, "the date day must parse");
                    assert_eq!(
                        dated.raw(),
                        [0xf2, 0xf0, 0xf0, 0xf0, 0xf0, 0xf2, 0xf2, 0xf9],
                        "the date must keep its raw EBCDIC zoned bytes"
                    );
                }
                _ => {
                    return Err(CopybookError::LengthMismatch { need: 1, have: 0 });
                }
            }
            assert_eq!(
                layout.encode(&back, &BOUNDS)?,
                record,
                "seed {seed}: decode then encode must return the same bytes"
            );
        }
    }
    Ok(())
}

#[test]
/// Hostile copybooks and records answer located diagnostics: a RENAMES
/// entry, an unknown clause, a truncation and a short record.
fn hostile_copybooks_answer_located_diagnostics() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        assert!(
            RecordLayout::parse("01 R.\n66 X RENAMES Y.\n", &BOUNDS).is_err(),
            "seed {seed}: RENAMES must be refused"
        );
        assert!(
            RecordLayout::parse("01 R.\n05 F PIC X(2) SYNC.\n", &BOUNDS).is_err(),
            "seed {seed}: an unknown clause must be refused"
        );
        let layout = match RecordLayout::parse(CUSTOMER, &BOUNDS) {
            Ok(drawn) => drawn,
            Err(_) => {
                continue;
            }
        };
        let cut = below(&mut state, layout.record_len());
        let short = vec![0x40_u8; cut];
        assert!(
            layout.decode(&short, &BOUNDS).is_err(),
            "seed {seed}: a record of {cut} of {} bytes must be refused",
            layout.record_len()
        );
    }
}

#[test]
/// The replay oracle: one seed, one trace.
fn the_same_seed_replays_to_the_same_copybook_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(copybook_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_copybook_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(copybook_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
