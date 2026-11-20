//! Seeded deterministic replay of the `id` UUID contracts.
//!
//! One seed draws every identifier, a failure prints the seed that produced it,
//! and the same seed must produce the same trace hash. Every probe drives the
//! public API of the shipped crate, and every answer is checked against a
//! reference parser and renderer written here from the documented canonical
//! form rather than from the implementation under test.
//!
//! The property INV-ID-1 names is that the v4 masks belong to **generation
//! only**: `new_v4` stamps the version and variant bits onto freshly drawn
//! entropy, while `parse` and the raw-byte constructor preserve whatever bits
//! they are handed. A parser that masked on the way in would silently rewrite
//! every non-v4 identifier a caller was trying to read back, and a family that
//! only ever generated ids would never notice — so this file checks arbitrary
//! drawn bytes through the whole parse/format/parse cycle.
//!
//! The module is gated `feature = "random"` because that is what gates
//! `lgwks_std::id` itself. INV-DEP-6: code in the feature matrix must not
//! assume the module exists.
#![cfg(feature = "random")]

use crate::seeded_bytes;
use crate::seeded_sweep;

use lgwks_std::id::{ParseError, Uuid};

use lgwks_std::seeded::Seeded;
use seeded_bytes::{fold_bytes, next_array, reference_lower_pair, reference_nibble};
use seeded_sweep::seeded_stream;
use seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, fold, fold_usize,
    initial_trace,
};

/// The refusal families: a malformed character, a missing separator, a wrong
/// length and a multi-byte scalar, each checked against the reference parser.
mod sim_refusals;

/// The canonical form is exactly this many characters.
const CANONICAL_LEN: usize = 36;

/// The hyphenated group layout, as (character width, byte width) per group.
const GROUPS: [(usize, usize); 5] = [(8, 4), (4, 2), (4, 2), (4, 2), (12, 6)];

/// The character offset at which each group's **hex** begins.
///
/// The separator is consumed before a non-first group's hex, so the hex of
/// group `n > 0` begins one character after that group's separator. This is the
/// offset INV-ID-1 names as the group start of a non-hex refusal.
const GROUP_STARTS: [usize; 5] = [0, 9, 14, 19, 24];

/// Renders 16 bytes as the canonical hyphenated lowercase form, computed from
/// the documented layout rather than by asking the shipped `Display`.
fn reference_render(bytes: &[u8; 16]) -> String {
    let mut rendered = String::with_capacity(CANONICAL_LEN);
    let mut cursor = 0_usize;
    for (index, &(_, byte_width)) in GROUPS.iter().enumerate() {
        if index > 0 {
            rendered.push('-');
        }
        // The group is read through `into_iter().flatten()`, which yields the
        // bytes or nothing: a group the layout table points past the end of a
        // sixteen-byte value is a table that no longer describes this renderer,
        // and an empty group says so rather than rendering a fallback digit.
        for byte in bytes
            .get(cursor..cursor.saturating_add(byte_width))
            .into_iter()
            .flatten()
        {
            rendered.push_str(&reference_lower_pair(*byte));
        }
        cursor = cursor.saturating_add(byte_width);
    }
    rendered
}

/// The trace arm for a parsed value that reports no version at all, kept
/// distinct from version zero so the two cannot fold alike.
const NO_VERSION_ARM: u64 = 0;

/// The trace arm for a version the parser reported.
///
/// A `u8` widens to the trace word without a checked conversion, which is why
/// the arms are `u64` rather than the width the version nibble is read at.
fn version_arm(value: &Uuid) -> u64 {
    match value.version() {
        Some(version) => u64::from(version).saturating_add(1),
        None => NO_VERSION_ARM,
    }
}

/// The character offset at which each group's hex begins, from the byte widths
/// alone.
///
/// The separator is consumed before a non-first group's hex, so the offset is
/// advanced past it before the hex start is recorded. This is derived rather
/// than copied from [`GROUP_STARTS`], so a wrong group start is a wrong answer
/// here instead of an echo of the shipped one — and the two must still agree,
/// which the malformed-hex family checks on every corrupted position.
fn reference_group_starts() -> [usize; 5] {
    let mut starts = [0usize; 5];
    let mut cursor = 0usize;
    for (slot, (index, &(char_width, _))) in starts.iter_mut().zip(GROUPS.iter().enumerate()) {
        if index > 0 {
            cursor = cursor.saturating_add(1);
        }
        *slot = cursor;
        cursor = cursor.saturating_add(char_width);
    }
    starts
}

/// The version nibble a raw value reports under the RFC variant rule.
fn reference_version(bytes: &[u8; 16]) -> Option<u8> {
    let eighth = bytes[8];
    if eighth & 0xc0 == 0x80 {
        bytes.get(6).map(|byte| byte >> 4)
    } else {
        None
    }
}

/// Parses canonical text into raw bytes, or names the refusal the documented
/// contract states.
///
/// Both coordinates of a non-hex refusal are produced: the group the offending
/// character belongs to, and the character itself. The group start is found by
/// walking the declared layout rather than by remembering a table, so a wrong
/// group start is a wrong answer here rather than a copy of the shipped one.
fn reference_parse(text: &str) -> Result<[u8; 16], Reference> {
    let bytes = text.as_bytes();
    if bytes.len() != CANONICAL_LEN {
        let refusal = Err(Reference::WrongLength { len: bytes.len() });
        #[cfg(feature = "trace")]
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "reference_parse: returning an error to the caller");
        return refusal;
    }
    let starts = reference_group_starts();
    let mut raw = [0u8; 16];
    let mut byte_cursor = 0usize;
    for (index, (&(_, byte_width), hex_at)) in GROUPS.iter().zip(starts).enumerate() {
        // The separator is consumed *before* a non-first group's hex, so it sits
        // one character earlier. Two different facts about two different
        // offsets: a missing separator is named at the separator's own
        // position, and a non-hex digit is named at its own position inside the
        // hex. Collapsing them is the mistake this separation exists to catch.
        let pair_start = if index > 0 {
            let hyphen_at = hex_at.saturating_sub(1);
            if bytes.get(hyphen_at) != Some(&b'-') {
                let refusal = Err(Reference::MissingHyphen { at: hyphen_at });
                #[cfg(feature = "trace")]
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "reference_parse: returning an error to the caller");
                return refusal;
            }
            hex_at
        } else {
            hex_at
        };

        for offset in 0..byte_width {
            let high_at = pair_start.saturating_add(offset.saturating_mul(2));
            let low_at = high_at.saturating_add(1);
            let high = bytes.get(high_at).copied().and_then(reference_nibble);
            let low = bytes.get(low_at).copied().and_then(reference_nibble);
            let at = match (high, low) {
                (Some(high), Some(low)) => {
                    let assembled = (high << 4) | low;
                    if let Some(slot) = raw.get_mut(byte_cursor.saturating_add(offset)) {
                        *slot = assembled;
                    }
                    continue;
                }
                (None, _) => high_at,
                (_, None) => low_at,
            };
            let refusal = Err(Reference::NotDigit {
                group_at: hex_at,
                at,
            });
            #[cfg(feature = "trace")]
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "reference_parse: returning an error to the caller");
            return refusal;
        }
        byte_cursor = byte_cursor.saturating_add(byte_width);
    }
    Ok(raw)
}

/// Why the reference parser refused one text. A successful parse yields the raw
/// bytes directly, so there is no "parsed" arm here to confuse a refusal with.
#[derive(Debug)]
enum Reference {
    /// The text was not 36 characters long.
    WrongLength {
        /// The observed length.
        len: usize,
    },
    /// A hyphen was missing before a group.
    MissingHyphen {
        /// Offset where the separator was required.
        at: usize,
    },
    /// A group held a character outside the hex alphabet.
    NotDigit {
        /// Offset where the malformed group begins.
        group_at: usize,
        /// Offset of the offending character itself.
        at: usize,
    },
}

/// The canonical form for arbitrary drawn bytes.
fn draw_canonical(state: &mut Seeded) -> String {
    reference_render(&next_array::<16>(state))
}

/// Runs the seeded parse/render sweep and returns its trace.
fn id_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();

    for _ in 0..64 {
        let raw = next_array::<16>(&mut state);
        let rendered = reference_render(&raw);

        assert_eq!(
            rendered.len(),
            CANONICAL_LEN,
            "seed {seed}: the canonical form is 36 characters"
        );
        let parsed = Uuid::parse(&rendered);
        assert_eq!(
            parsed.as_ref().map(Uuid::as_bytes),
            Ok(&raw),
            "seed {seed}: parsing must preserve every drawn bit"
        );
        assert_eq!(
            parsed.as_ref().map(ToString::to_string),
            Ok(rendered.clone()),
            "seed {seed}: the rendered form must round-trip through parse and display"
        );

        fold_bytes(&mut trace, &raw);
        fold_bytes(&mut trace, rendered.as_bytes());
        // The parse was asserted to agree with the drawn bytes above, so the
        // refusal arm is unreachable here; it still folds, because a fold that
        // assumed the assertion would be a fold of the assertion.
        let version = match parsed.as_ref() {
            Ok(value) => version_arm(value),
            Err(_) => NO_VERSION_ARM,
        };
        fold(&mut trace, version);
    }
    trace
}

#[test]
/// Arbitrary bytes — no version, no variant, every bit pattern — survive
/// `parse` then `Display` unchanged. This is INV-ID-1's parsing half: nothing
/// on the way in is masked.
fn arbitrary_bytes_round_trip_through_parse_and_display() -> Result<(), ParseError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..64 {
            let raw = next_array::<16>(&mut state);
            let rendered = reference_render(&raw);

            let parsed = Uuid::parse(&rendered)?;
            assert_eq!(
                parsed.as_bytes(),
                &raw,
                "seed {seed}: parse must preserve all sixteen drawn bytes"
            );
            assert_eq!(
                parsed.to_string(),
                rendered,
                "seed {seed}: display must render the drawn bytes as the reference does"
            );
            assert_eq!(
                Uuid::from(raw).to_string(),
                rendered,
                "seed {seed}: raw-byte construction must render like a parse"
            );
            assert_eq!(
                Uuid::from(raw),
                parsed,
                "seed {seed}: raw-byte construction and parsing must agree"
            );
        }
    }
    Ok(())
}

#[test]
/// The rendered form has exactly the documented layout: 36 characters, hyphens
/// at offsets 8, 13, 18 and 23, and hex digits everywhere else.
fn the_rendered_form_has_the_documented_hyphen_layout() -> Result<(), ParseError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let rendered = draw_canonical(&mut state);
            assert_eq!(
                rendered.len(),
                CANONICAL_LEN,
                "seed {seed}: the canonical form is 36 characters"
            );
            for hyphen_at in [8, 13, 18, 23] {
                assert_eq!(
                    rendered.as_bytes().get(hyphen_at),
                    Some(&b'-'),
                    "seed {seed}: a hyphen belongs at offset {hyphen_at}"
                );
            }
            for (index, character) in rendered.chars().enumerate() {
                let is_hyphen_slot = [8, 13, 18, 23].contains(&index);
                if is_hyphen_slot {
                    continue;
                }
                assert!(
                    character.is_ascii_hexdigit() && !character.is_ascii_uppercase(),
                    "seed {seed}: offset {index} holds {character:?}, which is not a lowercase hex digit"
                );
            }
            assert_eq!(
                Uuid::parse(&rendered)?.to_string(),
                rendered,
                "seed {seed}: a form of the documented layout parses and redisplayed identically"
            );
        }
    }
    Ok(())
}

#[test]
/// The v4 masks apply to generation only: `new_v4` stamps the version nibble
/// and the RFC variant bits, and every other byte of the identifier keeps the
/// entropy it was drawn from.
fn generated_identifiers_carry_the_v4_masks() -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..64 {
        let generated = Uuid::new_v4()?;
        let bytes = generated.as_bytes();
        assert_eq!(
            generated.version(),
            Some(4),
            "a generated identifier reports version 4"
        );
        assert_eq!(
            bytes.get(6).map(|byte| byte >> 4),
            Some(4),
            "the version nibble is stamped into the seventh byte"
        );
        assert_eq!(
            bytes.get(8).map(|byte| byte & 0xc0),
            Some(0x80),
            "the RFC variant bits are stamped into the ninth byte"
        );
    }
    Ok(())
}

#[test]
/// The masks are applied by generation and by nothing else, so every byte a
/// generator *did not* mask is free to be any value at all: a family that only
/// ever looked at versions would miss a parser that stamped one in.
fn parsing_preserves_version_and_variant_bits_a_generator_would_have_stamped() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            // The masks are stamped on the drawn array itself, so there is no
            // length conversion between the draw and the bytes that carry them.
            let mut raw = next_array::<16>(&mut state);
            raw[6] = (raw[6] & 0x0f) | 0x40;
            raw[8] = (raw[8] & 0x3f) | 0x80;
            let rendered = reference_render(&raw);

            let Ok(parsed) = Uuid::parse(&rendered) else {
                continue;
            };
            assert_eq!(
                parsed.as_bytes(),
                &raw,
                "seed {seed}: a v4-shaped value must parse back to the same bits"
            );
            assert_eq!(
                parsed.version(),
                Some(4),
                "seed {seed}: the parsed value reports the version its bits carry"
            );
        }
    }
}

#[test]
/// The version a value reports is a property of its own bits under the RFC
/// variant rule — `Some` only when the variant bits are set — and it is never
/// the value generation would have forced.
fn the_reported_version_follows_the_variant_rule_of_the_drawn_bits() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..48 {
            let raw = next_array::<16>(&mut state);
            let Ok(parsed) = Uuid::parse(&reference_render(&raw)) else {
                continue;
            };
            assert_eq!(
                parsed.version(),
                reference_version(&raw),
                "seed {seed}: the reported version must follow the variant rule of the drawn bits"
            );
        }
    }
}

#[test]
/// An upper-case spelling of a canonical form is the same identifier: the parse
/// accepts both cases and the display always renders lowercase.
fn an_upper_case_spelling_parses_to_the_same_identifier() -> Result<(), ParseError> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let rendered = draw_canonical(&mut state);
            let upper = rendered.to_ascii_uppercase();
            assert_eq!(
                upper.len(),
                rendered.len(),
                "seed {seed}: case must not change the length"
            );
            assert_eq!(
                Uuid::parse(&upper)?,
                Uuid::parse(&rendered)?,
                "seed {seed}: the two case spellings are one identifier"
            );
            assert_eq!(
                Uuid::parse(&upper)?.to_string(),
                rendered,
                "seed {seed}: an upper-case input still displays lowercase"
            );
        }
    }
    Ok(())
}

#[test]
/// The endpoints are exact. The all-zero value has no RFC variant and reports
/// no version; the all-one value has `0b11` in its variant bits, which is
/// neither of the two RFC variants, so it too reports none — a value whose
/// variant is unrecognisable has no version to report, rather than a fabricated
/// one read out of the wrong nibble.
fn the_all_zero_and_all_one_values_are_exact_endpoints() -> Result<(), ParseError> {
    let zero = Uuid::parse("00000000-0000-0000-0000-000000000000")?;
    assert_eq!(
        zero.as_bytes(),
        &[0u8; 16],
        "the all-zero value is sixteen zero bytes"
    );
    assert_eq!(
        zero.version(),
        None,
        "the all-zero value has no RFC variant and so reports no version"
    );
    assert_eq!(
        zero.to_string(),
        "00000000-0000-0000-0000-000000000000",
        "the all-zero value round-trips through its canonical form"
    );

    let ones = Uuid::parse("ffffffff-ffff-ffff-ffff-ffffffffffff")?;
    assert_eq!(
        ones.as_bytes(),
        &[0xffu8; 16],
        "the all-one value is sixteen set bytes"
    );
    assert_eq!(
        ones.version(),
        None,
        "the all-one value's variant bits are neither RFC variant, so it reports none"
    );
    assert_eq!(
        ones.to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "the all-one value round-trips through its canonical form"
    );
    Ok(())
}

#[test]
/// The replay oracle: one seed, one trace.
fn the_same_seed_replays_to_the_same_id_trace() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(id_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn distinct_id_seeds_diverge_in_their_trace() {
    assert_distinct_seeds_diverge(id_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}

#[test]
/// Two seeds draw two different identifiers, so the parse family is not one
/// value checked over and over.
fn two_seeds_draw_two_different_identifiers() {
    let mut first = seeded_stream(SWEEP_SEEDS[0]);
    let mut second = seeded_stream(SWEEP_SEEDS[1]);
    let left = draw_canonical(&mut first);
    let right = draw_canonical(&mut second);
    assert_ne!(
        left, right,
        "the two sweep seeds drew the same canonical form"
    );
    let mut trace = initial_trace();
    fold_usize(&mut trace, left.len());
    fold_bytes(&mut trace, left.as_bytes());
    assert_ne!(
        trace,
        initial_trace(),
        "the drawn identifier must fold into the trace"
    );
}
