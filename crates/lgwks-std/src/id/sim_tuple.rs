//! Seeded replay of the FoundationDB tuple element of [`Uuid`] (INV-ID-5, #851).
//!
//! The model is the tuple layer's specification (`design/tuple.md` in
//! apple/foundationdb): one byte `0x30`, then the 16 bytes in network order.
//! Every seed draws identifiers and trailing key bytes, checks the packed form
//! byte for byte against that model, reads it back with the trailing bytes left
//! to the caller, and holds the packed order to the identifiers' order, the
//! property the tuple layer chose network order for. The same seed gives the
//! same trace.

use super::tuple::{PACKED_LEN, UUID_CODE};
use super::{ParseError, TupleError, Uuid};
use crate::seeded::Seeded;

/// The seeds the sweeps replay: the estate's one named table.
const SEEDS: [u64; 4] = crate::seeded::SWEEP_SEEDS;

/// Identifier pairs drawn per seed.
const PER_SEED: usize = 4_096;

/// The mixing constant of the trace fold: the 64-bit FNV-1a prime.
const FNV_PRIME: u64 = 1_099_511_628_211;

/// The trace's starting value: the 64-bit FNV-1a offset basis.
const FNV_BASIS: u64 = 14_695_981_039_346_656_037;

/// RFC 9562 Appendix A.6, the version 7 example, as printed.
const RFC_V7: &str = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f";

/// The same example as the tuple layer packs it, written out by hand from the
/// specification: the code, then the identifier's bytes as printed.
const RFC_V7_PACKED: [u8; PACKED_LEN] = [
    0x30, 0x01, 0x7f, 0x22, 0xe2, 0x79, 0xb0, 0x7c, 0xc3, 0x98, 0xc4, 0xdc, 0x0c, 0x0c, 0x07, 0x39,
    0x8f,
];

/// One identifier of 128 drawn bits, no version imposed.
fn draw_uuid(stream: &mut Seeded) -> Uuid {
    let [h0, h1, h2, h3, h4, h5, h6, h7] = stream.next_u64().to_be_bytes();
    let [l0, l1, l2, l3, l4, l5, l6, l7] = stream.next_u64().to_be_bytes();
    Uuid::from([
        h0, h1, h2, h3, h4, h5, h6, h7, l0, l1, l2, l3, l4, l5, l6, l7,
    ])
}

/// The packed form the specification gives, built without the code under test.
fn model(id: Uuid) -> Vec<u8> {
    let mut packed = vec![0x30];
    packed.extend(id.as_bytes().iter().copied());
    packed
}

/// Folds every byte of `bytes` into the running trace.
fn fold_bytes(trace: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *trace = trace.wrapping_mul(FNV_PRIME).wrapping_add(u64::from(*byte));
    }
}

/// Every pair `seed` draws, packed after a drawn prefix and before a drawn
/// suffix; returns the trace of every packed key.
fn replay(seed: u64) -> Result<u64, TupleError> {
    let mut stream = Seeded::from_seed(seed);
    let mut trace = FNV_BASIS;
    for _ in 0..PER_SEED {
        let (first, second) = (draw_uuid(&mut stream), draw_uuid(&mut stream));
        let suffix = stream.next_u64().to_le_bytes();
        let mut key = Vec::new();
        first.pack_tuple(&mut key);
        assert_eq!(key, model(first), "seed {seed}");
        key.extend_from_slice(&suffix);
        let (read, rest) = Uuid::unpack_tuple(&key)?;
        assert_eq!((read, rest), (first, &suffix[..]), "seed {seed}");
        let mut other = Vec::new();
        second.pack_tuple(&mut other);
        assert_eq!(
            key.get(..PACKED_LEN).cmp(&other.get(..)),
            first.cmp(&second),
            "seed {seed}: packed order is identifier order"
        );
        fold_bytes(&mut trace, &key);
    }
    Ok(trace)
}

#[test]
fn the_rfc_vector_packs_to_the_bytes_the_specification_gives() -> Result<(), ParseError> {
    let id = Uuid::parse(RFC_V7)?;
    let mut key = Vec::new();
    id.pack_tuple(&mut key);
    assert_eq!(key, RFC_V7_PACKED);
    assert_eq!(Uuid::unpack_tuple(&RFC_V7_PACKED), Ok((id, &[][..])));
    Ok(())
}

#[test]
fn every_seed_packs_to_the_model_reads_back_and_keeps_the_order() -> Result<(), TupleError> {
    for seed in SEEDS {
        replay(seed)?;
    }
    Ok(())
}

#[test]
fn the_same_seed_replays_to_the_same_trace_and_seeds_diverge() -> Result<(), TupleError> {
    let [first, second, ..] = SEEDS;
    assert_eq!(replay(first)?, replay(first)?);
    assert_ne!(replay(first)?, replay(second)?);
    Ok(())
}

#[test]
fn every_truncation_and_every_other_code_is_refused_by_name()
-> Result<(), Box<dyn std::error::Error>> {
    let mut key = Vec::new();
    Uuid::parse(RFC_V7)?.pack_tuple(&mut key);
    for len in 0..PACKED_LEN {
        let cut = key.get(..len).ok_or("a prefix of the packed key")?;
        assert_eq!(
            Uuid::unpack_tuple(cut),
            Err(TupleError::Truncated { len }),
            "{len} bytes"
        );
    }
    for code in (0..=u8::MAX).filter(|code| *code != UUID_CODE) {
        let mut other = key.clone();
        if let Some(first) = other.first_mut() {
            *first = code;
        }
        assert_eq!(
            Uuid::unpack_tuple(&other),
            Err(TupleError::WrongCode { found: code }),
            "code {code:#04x}"
        );
    }
    Ok(())
}

#[test]
fn a_refusal_names_the_code_and_the_width() {
    let code = TupleError::WrongCode { found: 0x33 }.to_string();
    assert!(code.contains("0x33") && code.contains("0x30"), "{code}");
    let width = TupleError::Truncated { len: 3 }.to_string();
    assert!(width.contains('3') && width.contains("17"), "{width}");
}
