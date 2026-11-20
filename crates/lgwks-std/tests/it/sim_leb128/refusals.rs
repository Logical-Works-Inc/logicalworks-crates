//! The trailing-byte, minimality, truncation and overflow families of the
//! LEB128 sweep: every refusal arm against the reference model.

use super::*;

#[test]
/// The consumed prefix length leaves the trailing bytes to the caller: an
/// encoded value followed by arbitrary junk decodes to the value and the
/// length of its own encoding, and never inspects what follows.
fn trailing_bytes_are_left_to_the_caller() -> Result<(), DecodeError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..48 {
            let byte = next_byte(&mut state);
            let value = u64::from(byte).saturating_mul(2_654_435_761);
            let mut encoded = Vec::new();
            encode_u64(value, &mut encoded);
            let own_length = encoded.len();

            for extra in 0..8_usize {
                let mut with_trailer = encoded.clone();
                let trailer = next_bytes(&mut state, extra);
                with_trailer.extend_from_slice(&trailer);
                assert_eq!(
                    decode_u64(&with_trailer)?,
                    (value, own_length),
                    "seed {seed}: {extra} trailing bytes must not change the consumed length"
                );
            }

            // And the trailing bytes are the caller's to read: decoding the
            // value at an offset into the junk yields whatever the junk spells.
            let mut prefixed = trailer_prefixed(&mut state, &encoded);
            let own_start = prefixed.len().saturating_sub(encoded.len());
            assert_eq!(
                decode_u64(prefixed.split_at(own_start).1)?,
                (value, own_length),
                "seed {seed}: the value decodes at its own offset in a longer run"
            );
            prefixed.push(0x00);
        }
    }
    Ok(())
}

/// Returns `encoded` preceded by a few junk bytes, drawn from the sweep.
fn trailer_prefixed(state: &mut Seeded, encoded: &[u8]) -> Vec<u8> {
    let lead = below(state, 5);
    let mut prefixed = next_bytes(state, lead);
    prefixed.extend_from_slice(encoded);
    prefixed
}

#[test]
/// INV-LEB128-1's minimality: a redundant sign-extension group is refused, and
/// it is refused as **non-minimal** rather than as overflow or as a short
/// input — three different facts about a byte run that a family asserting one
/// arm for all of them would be conflating.
fn a_redundant_padding_group_is_refused_as_non_minimal() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..48 {
            let payload = next_byte(&mut state) % 0x7f;
            let mut over_padded = vec![payload | 0x80];
            over_padded.push(0x00);
            assert_eq!(
                decode_u64(&over_padded),
                Err(DecodeError::NonMinimal { at: 1 }),
                "seed {seed}: an unsigned padding group must be non-minimal"
            );
            assert_eq!(
                decode_u32(&over_padded),
                Err(DecodeError::NonMinimal { at: 1 }),
                "seed {seed}: the same at 32 bits"
            );

            let mut negative_padded = vec![(payload | 0x40) | 0x80];
            negative_padded.push(0x7f);
            assert_eq!(
                decode_i64(&negative_padded),
                Err(DecodeError::NonMinimal { at: 1 }),
                "seed {seed}: a signed 0x7f group after a negative one must be non-minimal"
            );
            assert_eq!(
                decode_i32(&negative_padded),
                Err(DecodeError::NonMinimal { at: 1 }),
                "seed {seed}: the same at 32 bits"
            );
        }
    }
}

#[test]
/// A run whose continuation bit is never cleared is refused as **truncated**,
/// at the length it was actually given — which is a different arm from
/// non-minimal and from overflow, even though the bytes may be the same ones a
/// padding group would have used.
///
/// The lengths stop short of the 64-bit group ceiling on purpose: a ten-byte
/// all-continuation run never reaches the truncation verdict at all, because the
/// tenth group is already past the width and the decoder refuses it as an
/// overflow first. That is the shipped order, and the boundary is where the two
/// arms are told apart.
fn an_unterminated_run_is_refused_as_truncated_at_its_own_length() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for length in 0..9_usize {
            let filler = next_byte(&mut state) | 0x80;
            let run = vec![filler; length];
            assert_eq!(
                decode_u64(&run),
                Err(DecodeError::UnexpectedEnd { at: length }),
                "seed {seed}: {length} continuation bytes is a truncated run, not a value"
            );
            assert_eq!(
                decode_i64(&run),
                Err(DecodeError::UnexpectedEnd { at: length }),
                "seed {seed}: the signed decoder reaches the same verdict"
            );
        }
    }
}

#[test]
/// A group that will not fit the target width is refused as **overflow**, at the
/// group that broke it — not at the end of the run, and not as a truncation.
/// The 32-bit and 64-bit decoders break at different groups for the same bytes,
/// which is the sharpest statement that the width is a real part of the
/// contract rather than a detail of the shift.
fn an_over_wide_run_is_refused_as_overflow_at_its_own_group() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let filler = next_byte(&mut state) | 0x80;
            let length = 4 + below(&mut state, 9);
            let run = vec![filler; length];

            agree!(
                seed,
                decode_u64(&run),
                reference_decode_unsigned(&run, 64),
                "wide run"
            );
            agree_narrow!(
                seed,
                decode_u32(&run),
                reference_decode_unsigned(&run, 32),
                "narrow run"
            );
            agree!(
                seed,
                decode_i64(&run),
                reference_decode_signed(&run, 64),
                "wide signed"
            );
            agree_narrow_signed!(
                seed,
                decode_i32(&run),
                reference_decode_signed(&run, 32),
                "narrow signed"
            );
        }
    }
}

#[test]
/// The width decides: the same byte run is refused at a 32-bit group where a
/// 64-bit decoder would still be reading, so a decoder cannot accept a value
/// its own width cannot hold.
fn the_target_width_decides_where_a_run_stops_being_valid() {
    // Nine continuation groups then a terminator: a 32-bit value can only use
    // five groups, so the 32-bit decoder must stop inside it.
    let mut narrow_only = vec![0x80u8; 9];
    narrow_only.push(0x01);
    assert_eq!(
        decode_u32(&narrow_only),
        Err(DecodeError::Overflow { at: 4 }),
        "a 32-bit unsigned decoder has only five usable groups"
    );

    let mut wide_ok = vec![0x80u8; 9];
    wide_ok.push(0x01);
    assert_eq!(
        decode_u64(&wide_ok),
        Ok((1u64 << 63, 10)),
        "a 64-bit unsigned decoder has ten, so the same bytes are a value"
    );

    let mut signed_narrow = vec![0x80u8; 5];
    signed_narrow.push(0x7f);
    assert_eq!(
        decode_i32(&signed_narrow),
        Err(DecodeError::Overflow { at: 4 }),
        "a 32-bit signed decoder stops at its fifth group too"
    );
}

#[test]
/// Every truncation of an encoded value is refused, and is refused on its own
/// terms rather than decoding to a shorter number: a cut that removes the
/// terminator is a truncated run.
fn every_truncation_of_an_encoding_is_refused() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let value = u64::from(next_byte(&mut state))
                .saturating_mul(2_654_435_761)
                .saturating_add(next_seed(&mut state).rem_euclid(97));
            let mut encoded = Vec::new();
            encode_u64(value, &mut encoded);
            for cut in 0..encoded.len() {
                // The run is bytes, so every offset in this range is a slice of
                // it; there is no character boundary to be on and no absent
                // prefix to substitute for.
                let prefix = encoded.split_at(cut).0;
                assert_eq!(
                    decode_u64(prefix),
                    Err(DecodeError::UnexpectedEnd { at: cut }),
                    "seed {seed}: a {cut}-byte truncation of a {}-byte encoding is truncated",
                    encoded.len()
                );
            }
        }
    }
}
