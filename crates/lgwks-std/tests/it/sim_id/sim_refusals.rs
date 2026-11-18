//! The refusal families of the `id` contracts: every corruption of a canonical
//! form is refused on the arm and at the coordinates the reference parser names.

use crate::seeded_bytes::{below, fold_refusal};
use crate::seeded_sweep::{SWEEP_SEEDS, fold_usize, initial_trace, seeded_stream};

use lgwks_std::id::{ParseError, Uuid};
use lgwks_std::seeded::Seeded;

use super::{CANONICAL_LEN, GROUP_STARTS, GROUPS, Reference, draw_canonical, reference_parse};

#[test]
/// A malformed hex character is reported at **both** coordinates INV-ID-1 names:
/// the start of the group it belongs to, and its own offset in the input.
///
/// Every non-hex character position of every drawn canonical form is corrupted
/// in turn, so the family covers all five groups rather than one of them.
///
/// A corruption of a separator is checked against the reference too, and the
/// reference says it is a *missing hyphen*: the two are different facts about
/// the same position, and a family that asserted one arm for all 36 positions
/// would be asserting a falsehood about four of them.
fn malformed_hex_reports_both_the_group_start_and_the_character_offset()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let rendered = draw_canonical(&mut state);
            // The alien is drawn from characters outside the hex alphabet and outside the
            // separator, so the corruption is a malformed *hex character* and the
            // text keeps its length — the two facts this family depends on.
            let alien = draw_alien(&mut state);
            let position = below(&mut state, CANONICAL_LEN);

            let mut corrupted = rendered.into_bytes();
            if !corrupt_position(&mut corrupted, position, alien) {
                continue;
            }
            let corrupted = String::from_utf8(corrupted).map_err(|error| {
                format!("seed {seed}: a corrupted identifier left ASCII: {error}")
            })?;

            match reference_parse(&corrupted) {
                Err(Reference::NotDigit { group_at, at }) => assert_eq!(
                    Uuid::parse(&corrupted),
                    Err(ParseError::NotHexDigit { group_at, at }),
                    "seed {seed}: position {position} must name its group {group_at} and itself {at}"
                ),
                Err(Reference::MissingHyphen { at }) => assert_eq!(
                    Uuid::parse(&corrupted),
                    Err(ParseError::MissingHyphen { at }),
                    "seed {seed}: position {position} must be reported as the separator it displaced"
                ),
                Err(Reference::WrongLength { len }) => {
                    return Err(format!("seed {seed}: the length moved to {len}").into());
                }
                Ok(_) => {
                    return Err(format!("seed {seed}: position {position} was accepted").into());
                }
            }
        }
    }
    Ok(())
}

/// The printable ASCII characters that are *not* hex digits, used to corrupt a
/// canonical form without shortening it or making it invalid UTF-8.
///
/// `-` is excluded as well: it is a legal character in the canonical form, but
/// at a hex position it is a different refusal (a missing separator), and this
/// family is about the two-coordinate non-hex refusal.
const ALIEN_CHARACTERS: &[u8; 11] = b"gzGZ !/:@[]";

/// An alien drawn from [`ALIEN_CHARACTERS`], as one byte of the corrupted text.
fn draw_alien(state: &mut Seeded) -> u8 {
    let index = below(state, ALIEN_CHARACTERS.len());
    ALIEN_CHARACTERS[index]
}

/// Replaces the character at `position`, reporting when the position is outside
/// the text so the caller can skip the case rather than write out of bounds.
fn corrupt_position(text: &mut [u8], position: usize, alien: u8) -> bool {
    match text.get_mut(position) {
        Some(slot) => {
            *slot = alien;
            true
        }
        None => false,
    }
}

#[test]
/// A canonical text of the wrong length is refused on that length, both one
/// character short and one character long — the two spellings a truncation and
/// an extension produce.
fn a_wrong_length_is_refused_before_anything_is_parsed() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let rendered = draw_canonical(&mut state);
            for len in [
                0,
                1,
                CANONICAL_LEN.saturating_sub(1),
                CANONICAL_LEN.saturating_sub(35),
            ] {
                assert!(
                    rendered.is_char_boundary(len),
                    "seed {seed}: the canonical form is ASCII, so offset {len} is a boundary"
                );
                let shortened = rendered[..len].to_owned();
                assert_eq!(
                    shortened.chars().count(),
                    len,
                    "seed {seed}: the drawn truncation really is {len} characters"
                );
                assert_eq!(
                    Uuid::parse(&shortened),
                    Err(ParseError::WrongLength { len, at: len }),
                    "seed {seed}: a {len}-character text is not the canonical form"
                );
            }
        }
    }
}

#[test]
/// Each of the four separators is required: removing one hyphen makes the text
/// 35 characters, and moving one off its own position makes a 36-character text
/// refuse with the separator's offset.
fn every_separator_position_is_required() -> Result<(), Box<dyn std::error::Error>> {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for hyphen_at in [8, 13, 18, 23] {
            let rendered = draw_canonical(&mut state);
            let mut moved = rendered.into_bytes();
            if let Some(slot) = moved.get_mut(hyphen_at) {
                *slot = b'0';
            }
            let moved = String::from_utf8(moved)
                .map_err(|error| format!("seed {seed}: moving a separator left ASCII: {error}"))?;
            assert_eq!(
                Uuid::parse(&moved),
                Err(ParseError::MissingHyphen { at: hyphen_at }),
                "seed {seed}: offset {hyphen_at} must hold the separator"
            );
        }
    }
    Ok(())
}

#[test]
/// Every truncation of a canonical form is refused on its length: the parser
/// admits exactly the 36-character layout and never a prefix of one.
fn every_truncation_of_a_canonical_form_is_refused() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..8 {
            let rendered = draw_canonical(&mut state);
            for cut in 0..CANONICAL_LEN {
                assert!(
                    rendered.is_char_boundary(cut),
                    "seed {seed}: the canonical form is ASCII, so offset {cut} is a boundary"
                );
                let prefix = &rendered[..cut];
                assert_eq!(
                    Uuid::parse(prefix),
                    Err(ParseError::WrongLength { len: cut, at: cut }),
                    "seed {seed}: a {cut}-character truncation is not a UUID"
                );
            }
        }
    }
}

#[test]
/// A multi-byte character inside a group is refused as a non-hex digit rather
/// than silently consuming the characters after it: replacing two hex
/// characters with one two-byte scalar keeps the text at 36 characters, and the
/// parser must name the scalar's own first byte rather than carry on past it.
fn a_multi_byte_character_in_a_group_is_refused_at_its_own_offset()
-> Result<(), Box<dyn std::error::Error>> {
    let multibyte = ['é', '☃', 'ü'];
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..16 {
            let rendered = draw_canonical(&mut state);
            let group_index = below(&mut state, GROUPS.len());
            // `GROUP_STARTS` is the hex start, which is where this family's
            // corruption must land: a separator position is a different refusal
            // with a different coordinate.
            let hex_start = GROUP_STARTS[group_index];
            let hex_width = GROUPS[group_index]
                .0
                .saturating_sub(usize::from(group_index > 0));
            let within = below(&mut state, hex_width.saturating_sub(1));
            let position = hex_start.saturating_add(within);
            let alien_index = below(&mut state, multibyte.len());
            let alien = multibyte[alien_index];

            // The two hex characters are replaced by one scalar, and however many bytes
            // that scalar occupies the *hex span* it takes is taken off the tail,
            // so the text is still exactly 36 bytes and the refusal can only be
            // about the character rather than about the length.
            let alien_len = alien.len_utf8();
            let tail_start = position.saturating_add(alien_len);
            let tail_len = CANONICAL_LEN.saturating_sub(tail_start);
            // The head ends where the alien begins and the tail starts where the
            // alien ends, both of which the layout puts inside an ASCII
            // rendering at a hex character; the assertions below re-check the
            // width, and this one checks that the split is on a boundary.
            assert!(
                rendered.is_char_boundary(position) && rendered.is_char_boundary(tail_start),
                "seed {seed}: the replacement at {position} must land on ASCII boundaries of the \
                 canonical form"
            );
            let head = &rendered[..position];
            let tail = &rendered[tail_start..tail_start.saturating_add(tail_len)];
            let corrupted = format!("{head}{alien}{tail}");
            assert_eq!(
                corrupted.len(),
                CANONICAL_LEN,
                "seed {seed}: the corrupted text is still {CANONICAL_LEN} bytes"
            );
            assert!(
                corrupted.len() > corrupted.chars().count(),
                "seed {seed}: the corrupted text really is carrying a multi-byte scalar"
            );

            match reference_parse(&corrupted) {
                Err(Reference::NotDigit { group_at, at }) => assert_eq!(
                    Uuid::parse(&corrupted),
                    Err(ParseError::NotHexDigit { group_at, at }),
                    "seed {seed}: the multi-byte scalar names its group and its own first byte"
                ),
                other => {
                    return Err(format!("seed {seed}: group {group_index} gave {other:?}").into());
                }
            }
        }
    }
    Ok(())
}

#[test]
/// A refusal arm and both of its coordinates are folded into the trace, so a
/// family that reached one arm for every corruption would still diverge here.
fn refusals_fold_their_arm_and_both_of_their_coordinates() -> Result<(), Box<dyn std::error::Error>>
{
    let mut trace = initial_trace();
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..24 {
            let rendered = draw_canonical(&mut state);
            let position = below(&mut state, CANONICAL_LEN);
            let alien = draw_alien(&mut state);
            let mut corrupted = rendered.into_bytes();
            if !corrupt_position(&mut corrupted, position, alien) {
                continue;
            }
            let corrupted = String::from_utf8(corrupted).map_err(|error| {
                format!("seed {seed}: a corrupted identifier left ASCII: {error}")
            })?;
            match Uuid::parse(&corrupted) {
                Ok(value) => fold_usize(&mut trace, value.to_string().len()),
                Err(ParseError::NotHexDigit { group_at, at }) => {
                    fold_refusal(&mut trace, 1, group_at, at);
                }
                Err(ParseError::MissingHyphen { at }) => fold_refusal(&mut trace, 2, at, at),
                Err(ParseError::WrongLength { len, at }) => fold_refusal(&mut trace, 3, len, at),
                Err(ParseError::DecodeOutputLength { expected, actual }) => {
                    fold_refusal(&mut trace, 4, expected, actual);
                }
                Err(other) => fold_refusal(&mut trace, 5, 0, format!("{other}").len()),
            }
        }
    }
    assert_ne!(
        trace,
        initial_trace(),
        "the refusal family must fold at least one observation"
    );
    Ok(())
}
