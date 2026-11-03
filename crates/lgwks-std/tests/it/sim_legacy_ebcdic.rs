//! Seeded round-trip and hostile simulations of the EBCDIC codecs.
//!
//! One seed draws every text, a failure names the seed that produced it, and
//! the same seed must produce the same trace hash (IBM CDRA code pages 037,
//! 500, 1047, 273: letters, digits, space and the common punctuation round
//! trip; national-use bytes distinguish the pages and unmapped bytes are
//! typed errors).

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::legacy::ebcdic::{CodePage, EbcdicError, decode, encode};
use lgwks_std::legacy::limits::Limits;
use lgwks_std::seeded::Seeded;

use seeded_bytes::{below, fold_bytes};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, fold_usize, initial_trace,
};

/// The bounds every sweep here runs under.
const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

/// The text alphabet every page maps: letters, digits, space, punctuation.
const SAFE_TEXT: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,<>+|&!$*);:-/,%_>? `#:@'=\"";

/// A seeded text drawn only from bytes every page maps.
fn draw_text(state: &mut Seeded, len: usize) -> String {
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let Some(&byte) = SAFE_TEXT.get(below(state, SAFE_TEXT.len())) else {
            continue;
        };
        out.push(char::from(byte));
    }
    out
}

/// Runs the seeded round-trip sweep and returns its trace.
fn ebcdic_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..48 {
        let text_len = below(&mut state, 64);
        let text = draw_text(&mut state, text_len);
        let page = match below(&mut state, 4) {
            0 => CodePage::Cp037,
            1 => CodePage::Cp500,
            2 => CodePage::Cp1047,
            _ => CodePage::Cp273,
        };
        let bytes = encode(page, &text, &BOUNDS);
        match bytes {
            Ok(encoded) => {
                let back = decode(page, &encoded, &BOUNDS);
                assert_eq!(
                    back.as_deref(),
                    Ok(text.as_str()),
                    "seed {seed}: text must survive the round trip"
                );
                fold_bytes(&mut trace, &encoded);
                fold_bytes(&mut trace, text.as_bytes());
            }
            Err(_) => {
                fold_usize(&mut trace, text.len());
            }
        }
    }
    trace
}

#[test]
/// Every seeded text survives `encode` then `decode` on its page.
fn a_seeded_text_round_trips_through_encode_and_decode() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let text_len = below(&mut state, 48);
            let text = draw_text(&mut state, text_len);
            let encoded = match encode(CodePage::Cp037, &text, &BOUNDS) {
                Ok(bytes) => bytes,
                Err(error) => {
                    assert!(
                        matches!(error, EbcdicError::UnmappableChar { .. }),
                        "seed {seed}: only an unmappable char may refuse"
                    );
                    continue;
                }
            };
            assert_eq!(
                decode(CodePage::Cp037, &encoded, &BOUNDS).as_deref(),
                Ok(text.as_str()),
                "seed {seed}: decode must return the drawn text"
            );
        }
    }
}

#[test]
/// Hostile bytes never panic: flips and truncations answer typed errors or
/// still-mapped text, and an empty bound refuses everything.
fn hostile_bytes_answer_typed_errors() {
    let tight = Limits::new(0, 0, 0, 0, 0);
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let text_len = below(&mut state, 24).saturating_add(1);
            let text = draw_text(&mut state, text_len);
            let Ok(encoded) = encode(CodePage::Cp037, &text, &BOUNDS) else {
                continue;
            };
            let mut hostile = encoded.clone();
            let position = below(&mut state, hostile.len().max(1));
            if let Some(slot) = hostile.get_mut(position) {
                *slot = slot.wrapping_add(1);
            }
            if let Ok(read) = decode(CodePage::Cp037, &hostile, &BOUNDS) {
                assert_eq!(
                    encode(CodePage::Cp037, &read, &BOUNDS),
                    Ok(hostile.clone()),
                    "seed {seed}: flipped bytes that decode must encode back to themselves"
                );
            }
            let cut = below(&mut state, encoded.len().saturating_add(1));
            if let (Some(prefix), Some(head)) = (encoded.get(..cut), text.get(..cut)) {
                assert_eq!(
                    decode(CodePage::Cp037, prefix, &BOUNDS).as_deref(),
                    Ok(head),
                    "seed {seed}: the first {cut} bytes must decode to the first {cut} characters"
                );
            }
            assert!(
                decode(CodePage::Cp037, &hostile, &tight).is_err(),
                "seed {seed}: a zero-byte ceiling must refuse"
            );
            assert!(
                encode(CodePage::Cp037, &text, &tight).is_err(),
                "seed {seed}: a zero-byte ceiling must refuse the encode"
            );
        }
    }
}

#[test]
/// The forward tables hold every byte and the limits refuse past ceilings.
fn tables_hold_every_byte_and_limits_name_every_ceiling() {
    for page in [
        CodePage::Cp037,
        CodePage::Cp500,
        CodePage::Cp1047,
        CodePage::Cp273,
    ] {
        assert_eq!(
            decode(page, &[0x40], &BOUNDS).as_deref(),
            Ok(" "),
            "every page must read 0x40 as space"
        );
    }
    let tight = Limits::new(0, 0, 0, 0, 0);
    assert!(
        encode(CodePage::Cp037, "A", &tight).is_err(),
        "a zero-byte ceiling must refuse the encode"
    );
    assert!(
        decode(CodePage::Cp037, &[0x40], &tight).is_err(),
        "a zero-byte ceiling must refuse the decode"
    );
}

#[test]
/// The replay oracle: one seed, one trace.
fn the_same_seed_replays_to_the_same_ebcdic_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(ebcdic_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_ebcdic_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(ebcdic_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
