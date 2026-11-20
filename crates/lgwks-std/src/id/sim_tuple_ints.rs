//! Seeded replay of the tuple layer's integer element against its specification.
//!
//! The model is `design/tuple.md` in apple/foundationdb, written here in `u128`
//! apart from the encoder: zero is `0x14`; a positive integer is `0x14 + n` and
//! its `n` big-endian bytes; a negative one is `0x14 - n` and the ones'
//! complement of its magnitude in `n` bytes. The encoder once read a failed
//! conversion as `0`, so a value it could not convert packed as another key;
//! these sims hold every width, sign and boundary to the model, the packed order
//! to the integer order, and a padded body to a refusal.

use super::tuple::{MAX_TUPLE_KEY_BYTES, pack_element, unpack_element};
use super::{Element, TupleError};
use crate::seeded::Seeded;

/// The seeds the sweeps replay: the estate's one named table.
const SEEDS: [u64; 4] = crate::seeded::SWEEP_SEEDS;

/// Integers drawn per seed.
const PER_SEED: usize = 2_048;

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The specification's bytes for `value`, computed without the encoder.
fn model(value: i64) -> Vec<u8> {
    if value == 0 {
        return vec![0x14];
    }
    let magnitude = u128::from(value.unsigned_abs());
    let mut width = 0_u8;
    let mut rest = magnitude;
    while rest > 0 {
        rest >>= 8_u32;
        width = width.saturating_add(1);
    }
    let all_ones = (1_u128 << (u32::from(width).saturating_mul(8))).saturating_sub(1);
    let (code, body) = if value > 0 {
        (0x14_u8.saturating_add(width), magnitude)
    } else {
        (
            0x14_u8.saturating_sub(width),
            all_ones.saturating_sub(magnitude),
        )
    };
    let mut key = vec![code];
    key.extend(
        body.to_be_bytes()
            .iter()
            .skip(16_usize.saturating_sub(usize::from(width))),
    );
    key
}

/// The encoder's bytes for `value`.
fn pack(value: i64) -> Result<Vec<u8>, TupleError> {
    let mut key = Vec::new();
    pack_element(&Element::Int(value), &mut key)?;
    Ok(key)
}

/// One integer of a drawn width: a 64-bit draw shifted right by 0 to 63 bits,
/// so every byte width is reached, with its sign as drawn.
fn draw(stream: &mut Seeded) -> i64 {
    // The top six bits of one draw are the shift, always below 64.
    let shift = stream.next_u64() >> 58_u32;
    let bits = stream.next_u64() >> shift;
    i64::from_be_bytes(bits.to_be_bytes())
}

#[test]
fn sim_tuple_ints_every_drawn_integer_packs_to_the_specification() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..PER_SEED {
            let value = draw(&mut stream);
            assert_eq!(pack(value)?, model(value), "seed {seed:#x}: {value}");
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_negated_draws_take_the_ones_complement_branch() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed ^ 0x4E47);
        for _ in 0..PER_SEED {
            let value = draw(&mut stream).saturating_abs().saturating_neg().min(-1);
            let key = pack(value)?;
            assert!(
                key.first().is_some_and(|code| *code < 0x14),
                "{value} packs below zero"
            );
            assert_eq!(key, model(value), "seed {seed:#x}: {value}");
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_every_width_boundary_on_both_signs_matches() -> TestResult {
    let mut edges = vec![0, 1, -1, i64::MAX, i64::MIN, i64::MIN.saturating_add(1)];
    for width in 1..8_u32 {
        let top = (1_i64 << width.saturating_mul(8)).saturating_sub(1);
        edges.extend([top, top.saturating_add(1), -top, -top.saturating_sub(1)]);
    }
    for value in edges {
        assert_eq!(pack(value)?, model(value), "boundary {value}");
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_the_packed_order_is_the_integer_order() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.rotate_left(17));
        for _ in 0..PER_SEED {
            let (left, right) = (draw(&mut stream), draw(&mut stream));
            assert_eq!(
                pack(left)?.cmp(&pack(right)?),
                left.cmp(&right),
                "seed {seed:#x}: {left} against {right}"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_a_packed_integer_reads_back_with_its_tail_left() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_mul(3));
        for _ in 0..PER_SEED {
            let value = draw(&mut stream);
            let mut key = pack(value)?;
            let tail = stream.next_u64().to_be_bytes();
            key.extend_from_slice(&tail);
            let (read, rest) = unpack_element(&key)?;
            assert_eq!(read, Element::Int(value), "seed {seed:#x}");
            assert_eq!(rest, tail, "seed {seed:#x}: the tail is the caller's");
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_the_saboteur_pads_a_body_and_is_refused() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(!seed);
        for _ in 0..PER_SEED {
            let value = draw(&mut stream);
            let honest = model(value);
            let Some((&code, body)) = honest.split_first() else {
                return Err("a packed integer has a code".into());
            };
            if body.len() >= 8 || value == 0 {
                continue;
            }
            let (wider, pad) = if value > 0 {
                (code.saturating_add(1), 0x00)
            } else {
                (code.saturating_sub(1), 0xff)
            };
            let mut padded = vec![wider, pad];
            padded.extend_from_slice(body);
            assert_eq!(
                unpack_element(&padded).map(|(element, _)| element),
                Err(TupleError::NonCanonicalInteger { code: wider }),
                "seed {seed:#x}: {value} padded to {padded:02x?}"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_a_body_cut_short_is_refused_as_truncated() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.rotate_right(9));
        for _ in 0..PER_SEED {
            let value = draw(&mut stream);
            let mut key = model(value);
            if key.len() < 2 {
                continue;
            }
            let keep = stream.index(key.len().saturating_sub(1))?.saturating_add(1);
            key.truncate(keep);
            assert!(
                matches!(unpack_element(&key), Err(TupleError::Truncated { .. })),
                "seed {seed:#x}: {value} cut to {key:02x?} is refused"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_every_width_code_with_no_body_is_refused() {
    for width in 1..=8_u8 {
        for code in [0x14_u8.saturating_add(width), 0x14_u8.saturating_sub(width)] {
            assert!(
                matches!(unpack_element(&[code]), Err(TupleError::Truncated { .. })),
                "code {code:#04x} with no body is refused"
            );
        }
    }
}

#[test]
fn sim_tuple_ints_the_double_clicker_packs_the_same_bytes_twice() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_add(0xD0));
        for _ in 0..PER_SEED {
            let value = draw(&mut stream);
            let mut key = Vec::new();
            pack_element(&Element::Int(value), &mut key)?;
            pack_element(&Element::Int(value), &mut key)?;
            let mut want = model(value);
            want.extend(model(value));
            assert_eq!(key, want, "seed {seed:#x}: {value} packed twice");
            let (first, rest) = unpack_element(&key)?;
            let (second, tail) = unpack_element(rest)?;
            assert_eq!((first, second), (Element::Int(value), Element::Int(value)));
            assert!(tail.is_empty(), "seed {seed:#x}: nothing past the second");
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_neighbours_in_one_buffer_read_back_in_order() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed ^ 0xA11);
        // Nine bytes at most per integer, so this many stay under the key cap.
        let fits = MAX_TUPLE_KEY_BYTES.div_euclid(9);
        let values: Vec<i64> = (0..fits).map(|_| draw(&mut stream)).collect();
        let mut key = Vec::new();
        for value in &values {
            pack_element(&Element::Int(*value), &mut key)?;
        }
        let want: Vec<u8> = values.iter().flat_map(|value| model(*value)).collect();
        assert_eq!(
            key, want,
            "seed {seed:#x}: the buffer is the models in order"
        );
        let mut rest = key.as_slice();
        for value in &values {
            let (read, after) = unpack_element(rest)?;
            assert_eq!(read, Element::Int(*value), "seed {seed:#x}");
            rest = after;
        }
        assert!(
            rest.is_empty(),
            "seed {seed:#x}: every byte was one integer's"
        );
        // The giant: keep packing until the cap refuses. Each success adds at
        // least one byte, so the loop ends within MAX_TUPLE_KEY_BYTES calls, and
        // the refused call leaves the buffer exactly as it was handed in.
        let (refused, kept) = loop {
            let kept = key.clone();
            if let Err(refusal) = pack_element(&Element::Int(draw(&mut stream)), &mut key) {
                break (refusal, kept);
            }
        };
        assert!(
            matches!(refused, TupleError::KeyTooLarge { .. }),
            "seed {seed:#x}: {refused:?}"
        );
        assert_eq!(key, kept, "seed {seed:#x}: a refusal rolls the buffer back");
        assert!(
            key.len() <= MAX_TUPLE_KEY_BYTES,
            "seed {seed:#x}: within the cap"
        );
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_adjacent_integers_pack_strictly_in_order() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_sub(0x0ff));
        for _ in 0..PER_SEED {
            let value = draw(&mut stream);
            let (Some(below), Some(above)) = (value.checked_sub(1), value.checked_add(1)) else {
                continue;
            };
            let (low, mid, high) = (pack(below)?, pack(value)?, pack(above)?);
            assert!(
                low < mid && mid < high,
                "seed {seed:#x}: {below} < {value} < {above}"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_tuple_ints_packing_appends_and_leaves_the_prefix_alone() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.rotate_left(29));
        for _ in 0..PER_SEED {
            let prefix = stream.next_u64().to_be_bytes();
            let value = draw(&mut stream);
            let mut key = prefix.to_vec();
            pack_element(&Element::Int(value), &mut key)?;
            let (head, packed) = key.split_at(prefix.len());
            assert_eq!(head, prefix, "seed {seed:#x}: the caller's bytes are kept");
            assert_eq!(
                packed,
                model(value),
                "seed {seed:#x}: {value} after the prefix"
            );
        }
    }
    Ok(())
}
