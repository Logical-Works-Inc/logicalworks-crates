//! The base64 door of the seeded `encoding` replay: round trip, truncation and
//! a corrupted character.

use super::{BOUNDARY_LENGTHS, SWEEP_SEEDS, base64, next_bytes, seeded_stream};

#[test]
/// Base64 round-trips every seeded payload and its rendering is the RFC 4648
/// shape: four characters per three input bytes, padded, and only characters
/// from the standard alphabet.
fn a_seeded_payload_round_trips_through_base64() -> Result<(), base64::DecodeError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for length in BOUNDARY_LENGTHS {
            let payload = next_bytes(&mut state, length);
            let encoded = base64::encode(&payload);
            let quantum = length.div_ceil(3).saturating_mul(4);
            assert_eq!(
                encoded.len(),
                quantum,
                "seed {seed}: a {length}-byte payload renders as {quantum} characters"
            );
            assert!(
                encoded.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || character == '+'
                        || character == '/'
                        || character == '='
                }),
                "seed {seed}: the rendering uses only the standard alphabet"
            );
            assert_eq!(
                base64::decode(&encoded)?,
                payload,
                "seed {seed}: a {length}-byte payload decodes back to itself"
            );
        }
    }
    Ok(())
}

#[test]
/// A base64 payload truncated by any number of characters is refused on its
/// length, never padded back out into the payload.
fn every_truncated_base64_prefix_is_refused_on_its_length() -> Result<(), base64::DecodeError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let payload = next_bytes(&mut state, 16);
            let encoded = base64::encode(&payload);
            for cut in 0..encoded.len() {
                // `cut` is a bound of this loop, so the prefix exists.
                let (prefix, _tail) = encoded.split_at(cut);
                if !cut.is_multiple_of(4) {
                    assert_eq!(
                        base64::decode(prefix),
                        Err(base64::DecodeError::BadLength { len: cut, at: cut }),
                        "seed {seed}: a {cut}-character prefix is not a whole quantum"
                    );
                } else {
                    assert_eq!(
                        base64::decode(prefix)?.len(),
                        cut.saturating_div(4).saturating_mul(3),
                        "seed {seed}: a whole quantum decodes to three bytes per four characters"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
/// A base64 payload with one character corrupted outside the alphabet is
/// refused at that character's own offset, and a good quantum beside it is
/// unaffected.
fn a_corrupted_base64_character_is_refused_at_its_offset() -> Result<(), Box<dyn std::error::Error>>
{
    for seed in SWEEP_SEEDS {
        for alien in [b'!', b'-', b'_', b' ', b'\n', 0x80, 0xff] {
            let good = base64::encode("standard base64 sample text");
            let padding_start = good.trim_end_matches('=').len();
            assert_eq!(
                base64::decode(&good)?.len(),
                good.len().saturating_div(4).saturating_mul(3),
                "seed {seed}: the good quantum decodes to its full length"
            );

            for position in 0..padding_start {
                let mut corrupted = good.clone().into_bytes();
                if let Some(slot) = corrupted.get_mut(position) {
                    *slot = alien;
                }
                assert_eq!(
                    base64::decode(&corrupted),
                    Err(base64::DecodeError::NotInAlphabet {
                        at: position,
                        byte: alien
                    }),
                    "seed {seed}: the body corruption at {position} is named at its own offset"
                );
            }

            for position in padding_start..good.len() {
                let mut corrupted = good.clone().into_bytes();
                if let Some(slot) = corrupted.get_mut(position) {
                    *slot = alien;
                }
                assert!(
                    base64::decode(&corrupted).is_err(),
                    "seed {seed}: a padding position holding {alien:#04x} must be refused"
                );
            }
        }
    }
    Ok(())
}
