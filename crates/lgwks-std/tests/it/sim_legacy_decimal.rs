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

/// The value a digit-by-digit saturating fold gives: the reference the
/// chunked decoder must match at every width, saturation included.
fn reference_fold(digits: &[u8]) -> i128 {
    digits.iter().fold(0_i128, |value, &digit| {
        value.saturating_mul(10).saturating_add(i128::from(digit))
    })
}

/// Packs `digits` as a COMP-3 field: a zero pad nibble for an even count,
/// the digits, then `sign`, two nibbles per byte (IBM Enterprise COBOL
/// numeric items, packed decimal).
fn pack_by_hand(digits: &[u8], sign: u8) -> Vec<u8> {
    let mut nibbles: Vec<u8> = Vec::with_capacity(digits.len().saturating_add(2));
    if digits.len().is_multiple_of(2) {
        nibbles.push(0);
    }
    nibbles.extend_from_slice(digits);
    nibbles.push(sign);
    nibbles
        .chunks(2)
        .map(|pair| match *pair {
            [high, low] => high.wrapping_mul(16) | low,
            [high] => high.wrapping_mul(16),
            _ => 0,
        })
        .collect()
}

#[test]
/// Every picture width from 1 to 41 digits decodes to what a digit-by-digit
/// saturating fold gives: inside one 19-digit chunk, across the chunk
/// boundaries at 19 and 38, and saturated past the 38 digits an `i128`
/// holds, for `C`, `D` and `F` signs (IBM Enterprise COBOL packed
/// decimal). Plain-text parsing of the same digits agrees, a second decode
/// of the same bytes agrees, and a non-digit nibble anywhere is refused at
/// its own byte.
fn sim_packed_digits_past_a_chunk_match_a_digit_by_digit_fold() {
    use lgwks_std::legacy::decimal::DecimalError;
    let wide = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for width in 1_usize..=41 {
            let digits: Vec<u8> = (0..width)
                .map(|_| {
                    let [digit, ..] = below(&mut state, 10).to_le_bytes();
                    digit
                })
                .collect();
            let magnitude = reference_fold(&digits);
            for (sign, signed_value) in [
                (0x0c_u8, magnitude),
                (0x0d, magnitude.saturating_neg()),
                (0x0f, magnitude),
            ] {
                let bytes = pack_by_hand(&digits, sign);
                let first = decode_packed(&bytes, width, 0, true, &wide);
                assert_eq!(
                    first,
                    Ok(Decimal::new(signed_value, 0)),
                    "seed {seed}: {width} digits with sign {sign:#x} must fold as digit by digit"
                );
                assert_eq!(
                    decode_packed(&bytes, width, 0, true, &wide),
                    first,
                    "seed {seed}: a second decode of the same {width}-digit field must agree"
                );
            }
            let text: String = digits
                .iter()
                .map(|&digit| char::from(b'0'.wrapping_add(digit)))
                .collect();
            assert_eq!(
                text.parse::<Decimal>(),
                Ok(Decimal::new(magnitude, 0)),
                "seed {seed}: {width} plain digits must parse as digit by digit"
            );
            let bad_at = below(&mut state, width);
            let mut hostile = digits.clone();
            if let Some(slot) = hostile.get_mut(bad_at) {
                *slot = 0x0a;
            }
            let pad = usize::from(width.is_multiple_of(2));
            assert_eq!(
                decode_packed(&pack_by_hand(&hostile, 0x0c), width, 0, true, &wide),
                Err(DecimalError::BadNibble {
                    offset: bad_at.saturating_add(pad).saturating_div(2),
                    nibble: 0x0a,
                }),
                "seed {seed}: digit {bad_at} of {width} set to A must be refused at its byte"
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
