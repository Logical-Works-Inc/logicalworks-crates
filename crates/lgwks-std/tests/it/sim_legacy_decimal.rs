//! Seeded round-trip and hostile simulations of the COBOL decimal codecs.
//!
//! One seed draws every coefficient, a failure names the seed that produced
//! it, and the same seed must produce the same trace hash (IBM Enterprise
//! COBOL Language Reference, numeric items: packed `COMP-3` nibbles with a
//! `C`/`D`/`F` sign, and zoned digits with an `F` or sign zone; bad nibbles
//! and signs are typed errors naming the byte).

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::decimal::{Decimal, decode_packed, encode_packed};
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, fold_bytes, next_byte};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, assert_verdict_replays,
    initial_trace,
};

/// The bounds every sweep here runs under.
const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

/// Ten to the `scale`, saturating rather than wrapping.
fn scale_factor(scale: u32) -> i128 {
    let mut factor: i128 = 1;
    for _ in 0..scale {
        factor = factor.saturating_mul(10);
    }
    factor
}

/// A seeded coefficient below `10^18` with a seeded sign.
fn draw_coefficient(state: &mut Seeded) -> i128 {
    let mut magnitude: i128 = 0;
    for _ in 0..4 {
        magnitude = magnitude
            .saturating_mul(256)
            .saturating_add(i128::from(next_byte(state)));
    }
    let bounded = magnitude.rem_euclid(scale_factor(18));
    if below(state, 2) == 0 {
        bounded
    } else {
        bounded.saturating_neg()
    }
}

/// Runs the seeded round-trip sweep and returns its trace.
fn decimal_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..48 {
        let coefficient = draw_coefficient(&mut state);
        let scale = match u32::try_from(below(&mut state, 5)) {
            Ok(drawn) => drawn,
            Err(_) => continue,
        };
        let value = Decimal::new(coefficient, scale);
        if let Ok(packed) = encode_packed(value, 18, true, &BOUNDS) {
            let back = decode_packed(&packed, 18, scale, true, &BOUNDS);
            assert_eq!(
                back,
                Ok(value),
                "seed {seed}: packed must survive the round trip"
            );
            fold_bytes(&mut trace, &packed);
        }
    }
    trace
}

#[test]
/// The fixed vectors: 12345.67 packs to `12 34 56 7C` at seven digits.
fn the_packed_layout_matches_its_vector() -> Result<(), lgwks_std::legacy::decimal::DecimalError> {
    let value = Decimal::new(1_234_567, 2);
    let bytes = encode_packed(value, 7, true, &BOUNDS)?;
    assert_eq!(
        bytes,
        vec![0x12, 0x34, 0x56, 0x7c],
        "12345.67 packed must be 12 34 56 7C"
    );
    assert_eq!(
        decode_packed(&bytes, 7, 2, true, &BOUNDS)?,
        value,
        "the packed bytes must decode back"
    );
    Ok(())
}

#[test]
/// An unsigned picture packs with the `F` sign IBM COBOL writes for it, a
/// signed one keeps `C` and `D`, and every form decodes back. Each seeded
/// magnitude is packed both ways, so the sign nibble is the only byte the
/// two encodings may differ in.
fn an_unsigned_packed_value_carries_the_f_sign()
-> Result<(), lgwks_std::legacy::decimal::DecimalError> {
    let unsigned = encode_packed(Decimal::new(789, 0), 3, false, &BOUNDS)?;
    assert_eq!(
        unsigned,
        vec![0x78, 0x9f],
        "PIC 9(3) COMP-3 789 must be 78 9F"
    );
    assert_eq!(
        encode_packed(Decimal::new(789, 0), 3, true, &BOUNDS)?,
        vec![0x78, 0x9c],
        "PIC S9(3) COMP-3 +789 must be 78 9C"
    );
    assert_eq!(
        decode_packed(&unsigned, 3, 0, false, &BOUNDS)?,
        Decimal::new(789, 0),
        "the F-signed bytes must decode back unsigned"
    );
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..24 {
            let magnitude = draw_coefficient(&mut state).saturating_abs();
            let digits = below(&mut state, 18).saturating_add(1);
            let Ok(plain) = encode_packed(Decimal::new(magnitude, 0), digits, false, &BOUNDS)
            else {
                continue;
            };
            let signed = encode_packed(Decimal::new(magnitude, 0), digits, true, &BOUNDS)?;
            let (Some((&plain_last, plain_digits)), Some((&signed_last, signed_digits))) =
                (plain.split_last(), signed.split_last())
            else {
                continue;
            };
            assert_eq!(
                plain_digits, signed_digits,
                "seed {seed}: only the sign nibble may differ between the two encodings"
            );
            assert_eq!(
                (plain_last & 0x0f, signed_last & 0x0f),
                (0x0f, 0x0c),
                "seed {seed}: unsigned {magnitude} must end F and signed must end C"
            );
            assert_eq!(
                decode_packed(&plain, digits, 0, false, &BOUNDS)?,
                Decimal::new(magnitude, 0),
                "seed {seed}: unsigned {magnitude} in {digits} digits must decode back"
            );
        }
    }
    Ok(())
}

#[test]
/// Hostile nibbles and signs answer typed errors: a digit past 9, a sign
/// past `C`/`D`/`F`, a truncation and a zero-byte ceiling.
fn hostile_nibbles_answer_typed_errors() {
    let tight = Limits::new(0, 0, 0, 0, 0);
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..24 {
            let value = Decimal::new(draw_coefficient(&mut state), 0);
            let Ok(bytes) = encode_packed(value, 18, true, &BOUNDS) else {
                continue;
            };
            let mut hostile = bytes.clone();
            let position = below(&mut state, hostile.len().max(1));
            if let Some(slot) = hostile.get_mut(position) {
                *slot = slot.wrapping_add(0x1a);
            }
            assert_verdict_replays(seed, "flipped packed value", || {
                decode_packed(&hostile, 18, 0, true, &BOUNDS)
            });
            let cut = below(&mut state, bytes.len());
            if let Some(prefix) = bytes.get(..cut) {
                assert!(
                    decode_packed(prefix, 18, 0, true, &BOUNDS).is_err(),
                    "seed {seed}: a packed value cut to {cut} of {} bytes must be refused",
                    bytes.len()
                );
            }
            assert!(
                decode_packed(&bytes, 18, 0, true, &tight).is_err(),
                "seed {seed}: a zero-byte ceiling must refuse"
            );
        }
    }
}

#[test]
/// The replay oracle: one seed, one trace.
fn the_same_seed_replays_to_the_same_decimal_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(decimal_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_decimal_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(decimal_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
