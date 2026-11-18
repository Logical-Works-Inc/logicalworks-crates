//! The constant-time comparison of digests (#275): `ct_eq` is the same
//! equality `==` and `Ord` answer, sees every bit, and never collapses keys.

use crate::seeded_bytes::{below, next_byte, next_bytes};
use crate::seeded_sweep::{SWEEP_SEEDS, seeded_stream};

use lgwks_std::hash::{Digest, DigestParseError, blake3, keyed};

use super::BOUNDARY_LENGTHS;

/// Seeded digests every `ct_eq` family compares: one per boundary length and
/// 32 drawn messages, each paired with a byte-equal copy built from its bytes.
fn ct_eq_pairs(seed: u64) -> Vec<(Digest, Digest)> {
    let mut state = seeded_stream(seed);
    let mut pairs = Vec::new();
    for length in BOUNDARY_LENGTHS {
        let digest = blake3(&next_bytes(&mut state, length));
        pairs.push((digest, Digest::from_bytes(*digest.as_bytes())));
    }
    for _ in 0..32 {
        let length = below(&mut state, 96);
        let digest = blake3(&next_bytes(&mut state, length));
        pairs.push((digest, Digest::from_bytes(*digest.as_bytes())));
    }
    pairs
}

#[test]
/// `ct_eq` answers exactly what `==` answers, on equal copies and on every
/// unequal pairing of seeded digests (#275): the constant-time path is the
/// same comparison, not a second definition of equality.
fn ct_eq_agrees_with_equality_on_seeded_pairs() {
    for seed in SWEEP_SEEDS {
        let pairs = ct_eq_pairs(seed);
        for &(left, copy) in &pairs {
            assert!(left.ct_eq(&copy), "seed {seed}: a byte-equal copy is ct_eq");
            for &(other, _) in &pairs {
                assert_eq!(
                    left.ct_eq(&other),
                    left == other,
                    "seed {seed}: ct_eq and == must agree on every pairing"
                );
            }
        }
    }
}

#[test]
/// `ct_eq` is reflexive and symmetric over seeded digests.
fn ct_eq_is_reflexive_and_symmetric() {
    for seed in SWEEP_SEEDS {
        let pairs = ct_eq_pairs(seed);
        for &(left, _) in &pairs {
            assert!(
                left.ct_eq(&left),
                "seed {seed}: a digest is ct_eq to itself"
            );
            for &(right, _) in &pairs {
                assert_eq!(
                    left.ct_eq(&right),
                    right.ct_eq(&left),
                    "seed {seed}: ct_eq must not depend on argument order"
                );
            }
        }
    }
}

#[test]
/// A single flipped bit at a seeded byte and bit is seen by `ct_eq`, so no
/// position of the 32 bytes is skipped by the constant-time comparison.
fn ct_eq_sees_a_flip_at_every_seeded_position() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for (digest, _) in ct_eq_pairs(seed) {
            for _ in 0..64 {
                let byte = below(&mut state, 32);
                // `below` draws in `0..8`, so the bit index is a shift of a byte
                // by at most seven and is never a width the shift cannot take.
                let bit = below(&mut state, 8);
                let mut flipped = *digest.as_bytes();
                if let Some(slot) = flipped.get_mut(byte) {
                    *slot ^= 1_u8 << bit;
                }
                let changed = Digest::from_bytes(flipped);
                assert!(
                    !digest.ct_eq(&changed) && !changed.ct_eq(&digest),
                    "seed {seed}: a flip of bit {bit} in byte {byte} must be seen"
                );
            }
        }
    }
}

#[test]
/// `Ord` reports `Equal` exactly when `ct_eq` holds, including for digests that
/// share every byte but the last.
fn ordering_is_equal_exactly_when_ct_eq_holds() {
    for seed in SWEEP_SEEDS {
        let pairs = ct_eq_pairs(seed);
        for &(left, copy) in &pairs {
            let mut tail = *left.as_bytes();
            if let Some(last) = tail.last_mut() {
                *last ^= 0x80;
            }
            let neighbour = Digest::from_bytes(tail);
            for right in [copy, neighbour] {
                assert_eq!(
                    left.cmp(&right) == std::cmp::Ordering::Equal,
                    left.ct_eq(&right),
                    "seed {seed}: Ord's Equal and ct_eq must agree"
                );
            }
        }
    }
}

#[test]
/// A digest parsed back from its hex form, in either case, is `ct_eq` to the
/// original, so a verifier reading a recorded head compares like with like.
fn a_digest_parsed_from_its_hex_is_ct_eq() -> Result<(), DigestParseError> {
    for seed in SWEEP_SEEDS {
        for (digest, _) in ct_eq_pairs(seed) {
            let hex = digest.to_hex();
            assert!(
                Digest::from_hex(&hex)?.ct_eq(&digest),
                "seed {seed}: the lower-case hex parses to a ct_eq digest"
            );
            assert!(
                Digest::from_hex(&hex.to_ascii_uppercase())?.ct_eq(&digest),
                "seed {seed}: the upper-case hex parses to a ct_eq digest"
            );
        }
    }
    Ok(())
}

#[test]
/// `ct_eq` separates a keyed digest from the unkeyed digest of the same message
/// and from the same message under another key: equality never collapses
/// across the key that a tenant's records are bound to.
fn ct_eq_separates_keys_over_one_message() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..32 {
            let length = below(&mut state, 96);
            let message = next_bytes(&mut state, length);
            let mut first = [0_u8; 32];
            first
                .iter_mut()
                .for_each(|byte| *byte = next_byte(&mut state));
            let mut second = first;
            if let Some(byte) = second.first_mut() {
                *byte ^= 1;
            }
            let plain = blake3(&message);
            let under_first = keyed(&first, &message);
            let under_second = keyed(&second, &message);
            assert!(
                !under_first.ct_eq(&plain),
                "seed {seed}: a keyed digest is not the unkeyed one"
            );
            assert!(
                !under_first.ct_eq(&under_second),
                "seed {seed}: two keys give two digests of one message"
            );
            assert!(
                under_first.ct_eq(&keyed(&first, &message)),
                "seed {seed}: one key and one message give one digest"
            );
        }
    }
}
