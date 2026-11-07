//! Seeded replay of the FoundationDB tuple element of [`Uuid`] (INV-ID-5, #851).
//!
//! The model is the tuple layer's specification (`design/tuple.md` in
//! apple/foundationdb): one byte `0x30`, then the 16 bytes in network order.
//! Every seed draws identifiers and trailing key bytes, checks the packed form
//! byte for byte against that model, reads it back with the trailing bytes left
//! to the caller, and holds the packed order to the identifiers' order, the
//! property the tuple layer chose network order for. The same seed gives the
//! same trace.

use std::borrow::Cow;

use super::tuple::{
    MAX_TUPLE_DEPTH, MAX_TUPLE_ELEMENTS, MAX_TUPLE_KEY_BYTES, PACKED_LEN, UUID_CODE, pack_element,
    pack_elements, unpack_element, unpack_elements,
};
use super::{Element, ParseError, TupleError, Uuid};
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

#[test]
fn sim_tuple_full_all_twelve_typecodes_pack_and_unpack_correctly()
-> Result<(), Box<dyn std::error::Error>> {
    // 1. Null
    let mut null_key = Vec::new();
    pack_element(&Element::Null, &mut null_key)?;
    assert_eq!(null_key, &[0x00]);
    let (null_elem, null_rest) = unpack_element(&null_key)?;
    assert_eq!(null_elem, Element::Null);
    assert!(null_rest.is_empty());

    // 2. Bytes with embedded 0x00
    let raw_bytes: &[u8] = &[0xAA, 0x00, 0xBB];
    let mut bytes_key = Vec::new();
    pack_element(&Element::Bytes(Cow::Borrowed(raw_bytes)), &mut bytes_key)?;
    assert_eq!(bytes_key, &[0x01, 0xAA, 0x00, 0xFF, 0xBB, 0x00]);
    let (bytes_elem, bytes_rest) = unpack_element(&bytes_key)?;
    assert_eq!(bytes_elem, Element::Bytes(Cow::Borrowed(raw_bytes)));
    assert!(bytes_rest.is_empty());

    // 3. String with embedded \0
    let raw_str = "hello\0world";
    let mut str_key = Vec::new();
    pack_element(&Element::String(Cow::Borrowed(raw_str)), &mut str_key)?;
    assert_eq!(
        str_key,
        &[
            0x02, b'h', b'e', b'l', b'l', b'o', 0x00, 0xFF, b'w', b'o', b'r', b'l', b'd', 0x00
        ]
    );
    let (str_elem, str_rest) = unpack_element(&str_key)?;
    assert_eq!(str_elem, Element::String(Cow::Borrowed(raw_str)));
    assert!(str_rest.is_empty());

    // 4. Nested Tuple with Null
    let nested = Element::Tuple(vec![Element::Null, Element::Int(42)]);
    let mut tuple_key = Vec::new();
    pack_element(&nested, &mut tuple_key)?;
    assert_eq!(tuple_key, &[0x05, 0x00, 0xFF, 0x15, 42, 0x00]);
    let (tuple_elem, tuple_rest) = unpack_element(&tuple_key)?;
    assert_eq!(tuple_elem, nested);
    assert!(tuple_rest.is_empty());

    // 5. Int across boundary values: i64::MIN, -256, -255, -1, 0, 1, 255, 256, i64::MAX
    let mut min_key = Vec::new();
    pack_element(&Element::Int(i64::MIN), &mut min_key)?;
    assert_eq!(
        min_key,
        &[0x0c, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
    );

    let mut n256_key = Vec::new();
    pack_element(&Element::Int(-256), &mut n256_key)?;
    assert_eq!(n256_key, &[0x12, 0xfe, 0xff]);

    let mut n255_key = Vec::new();
    pack_element(&Element::Int(-255), &mut n255_key)?;
    assert_eq!(n255_key, &[0x13, 0x00]);

    let mut n1_key = Vec::new();
    pack_element(&Element::Int(-1), &mut n1_key)?;
    assert_eq!(n1_key, &[0x13, 0xfe]);

    let mut zero_key = Vec::new();
    pack_element(&Element::Int(0), &mut zero_key)?;
    assert_eq!(zero_key, &[0x14]);

    let mut p1_key = Vec::new();
    pack_element(&Element::Int(1), &mut p1_key)?;
    assert_eq!(p1_key, &[0x15, 0x01]);

    let mut p255_key = Vec::new();
    pack_element(&Element::Int(255), &mut p255_key)?;
    assert_eq!(p255_key, &[0x15, 0xff]);

    let mut p256_key = Vec::new();
    pack_element(&Element::Int(256), &mut p256_key)?;
    assert_eq!(p256_key, &[0x16, 0x01, 0x00]);

    let mut max_key = Vec::new();
    pack_element(&Element::Int(i64::MAX), &mut max_key)?;
    assert_eq!(
        max_key,
        &[0x1c, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
    );

    let boundary_ints = [i64::MIN, -256, -255, -1, 0, 1, 255, 256, i64::MAX];
    let mut prev_int_key: Option<Vec<u8>> = None;
    for int_val_ref in &boundary_ints {
        let int_val = *int_val_ref;
        let mut key_buf = Vec::new();
        pack_element(&Element::Int(int_val), &mut key_buf)?;
        let (unpacked, rest) = unpack_element(&key_buf)?;
        assert_eq!(unpacked, Element::Int(int_val));
        assert!(rest.is_empty());
        if let Some(ref prev) = prev_int_key {
            assert!(
                *prev < key_buf,
                "int ordering failed: {prev:?} < {key_buf:?}"
            );
        }
        prev_int_key = Some(key_buf);
    }

    // 6. F32
    let boundary_f32s = [-1000.5f32, -1.0f32, -0.0f32, 0.0f32, 1.0f32, 1000.5f32];
    let mut prev_f32_key: Option<Vec<u8>> = None;
    for float_ref in &boundary_f32s {
        let float_val = *float_ref;
        let mut key_buf = Vec::new();
        pack_element(&Element::F32(float_val), &mut key_buf)?;
        let (unpacked, rest) = unpack_element(&key_buf)?;
        assert_eq!(unpacked, Element::F32(float_val));
        assert!(rest.is_empty());
        if let Some(ref prev) = prev_f32_key {
            assert!(
                *prev < key_buf,
                "f32 ordering failed: {prev:?} < {key_buf:?}"
            );
        }
        prev_f32_key = Some(key_buf);
    }

    // 7. F64
    let boundary_f64s = [-1000.5f64, -1.0f64, -0.0f64, 0.0f64, 1.0f64, 1000.5f64];
    let mut prev_f64_key: Option<Vec<u8>> = None;
    for float_ref in &boundary_f64s {
        let float_val = *float_ref;
        let mut key_buf = Vec::new();
        pack_element(&Element::F64(float_val), &mut key_buf)?;
        let (unpacked, rest) = unpack_element(&key_buf)?;
        assert_eq!(unpacked, Element::F64(float_val));
        assert!(rest.is_empty());
        if let Some(ref prev) = prev_f64_key {
            assert!(
                *prev < key_buf,
                "f64 ordering failed: {prev:?} < {key_buf:?}"
            );
        }
        prev_f64_key = Some(key_buf);
    }

    // 8. Bool
    let mut bool_f_key = Vec::new();
    pack_element(&Element::Bool(false), &mut bool_f_key)?;
    assert_eq!(bool_f_key, &[0x26]);
    let (unpacked_bf, _) = unpack_element(&bool_f_key)?;
    assert_eq!(unpacked_bf, Element::Bool(false));

    let mut bool_t_key = Vec::new();
    pack_element(&Element::Bool(true), &mut bool_t_key)?;
    assert_eq!(bool_t_key, &[0x27]);
    let (unpacked_bt, _) = unpack_element(&bool_t_key)?;
    assert_eq!(unpacked_bt, Element::Bool(true));
    assert!(bool_f_key < bool_t_key);

    // 9. Uuid
    let id_val = Uuid::parse(RFC_V7)?;
    let mut uuid_key = Vec::new();
    pack_element(&Element::Uuid(id_val), &mut uuid_key)?;
    assert_eq!(uuid_key, RFC_V7_PACKED);
    let (unpacked_uuid, _) = unpack_element(&uuid_key)?;
    assert_eq!(unpacked_uuid, Element::Uuid(id_val));

    // 10. Versionstamp80
    let vs80 = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let mut vs80_key = Vec::new();
    pack_element(&Element::Versionstamp80(vs80), &mut vs80_key)?;
    assert_eq!(vs80_key.first(), Some(&0x32));
    assert_eq!(vs80_key.len(), 11);
    let (unpacked_vs80, _) = unpack_element(&vs80_key)?;
    assert_eq!(unpacked_vs80, Element::Versionstamp80(vs80));

    // 11. Versionstamp96
    let vs96 = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let mut vs96_key = Vec::new();
    pack_element(&Element::Versionstamp96(vs96), &mut vs96_key)?;
    assert_eq!(vs96_key.first(), Some(&0x33));
    assert_eq!(vs96_key.len(), 13);
    let (unpacked_vs96, _) = unpack_element(&vs96_key)?;
    assert_eq!(unpacked_vs96, Element::Versionstamp96(vs96));

    Ok(())
}

#[test]
fn sim_tuple_full_depth_element_and_size_bounds_refused() {
    // 1. Depth bound exceeded (> MAX_TUPLE_DEPTH = 32)
    let mut deep = Element::Null;
    for _ in 0..=MAX_TUPLE_DEPTH {
        deep = Element::Tuple(vec![deep]);
    }
    let mut deep_key = Vec::new();
    assert_eq!(
        pack_element(&deep, &mut deep_key),
        Err(TupleError::DepthExceeded {
            limit: MAX_TUPLE_DEPTH
        })
    );

    // 2. Element count exceeded (> MAX_TUPLE_ELEMENTS = 1,024)
    let mut too_many = Vec::with_capacity(MAX_TUPLE_ELEMENTS.saturating_add(1));
    for _ in 0..=MAX_TUPLE_ELEMENTS {
        too_many.push(Element::Int(0));
    }
    let mut too_many_key = Vec::new();
    assert_eq!(
        pack_elements(&too_many, &mut too_many_key),
        Err(TupleError::TooManyElements {
            limit: MAX_TUPLE_ELEMENTS
        })
    );

    // 3. Key too large (> MAX_TUPLE_KEY_BYTES = 10,000)
    let huge_bytes = vec![0x55u8; MAX_TUPLE_KEY_BYTES.saturating_add(10)];
    let huge_elem = Element::Bytes(Cow::Borrowed(&huge_bytes));
    let mut huge_key = Vec::new();
    assert_eq!(
        pack_element(&huge_elem, &mut huge_key),
        Err(TupleError::KeyTooLarge {
            limit: MAX_TUPLE_KEY_BYTES
        })
    );
}

#[test]
fn sim_tuple_ordered_sweep() -> Result<(), TupleError> {
    sim_fdb_tuple_element_lexicographical_bijection()
}

#[test]
fn sim_fdb_tuple_element_lexicographical_bijection() -> Result<(), TupleError> {
    let [first_seed, ..] = SEEDS;
    let mut stream = Seeded::from_seed(first_seed);
    let count = 5_000_usize;
    let mut elements_list = Vec::with_capacity(count);
    let mut packed_list = Vec::with_capacity(count);

    for _ in 0..count {
        let val_int = i64::from_ne_bytes(stream.next_u64().to_ne_bytes());
        let mut rem_0 = 0_u8;
        if let Some(rem_val) = stream.next_u64().checked_rem(26)
            && let Ok(byte_val) = u8::try_from(rem_val)
        {
            rem_0 = byte_val;
        }
        let mut rem_1 = 0_u8;
        if let Some(rem_val) = stream.next_u64().checked_rem(26)
            && let Ok(byte_val) = u8::try_from(rem_val)
        {
            rem_1 = byte_val;
        }
        let mut rem_2 = 0_u8;
        if let Some(rem_val) = stream.next_u64().checked_rem(26)
            && let Ok(byte_val) = u8::try_from(rem_val)
        {
            rem_2 = byte_val;
        }
        let char_0 = b'a'.saturating_add(rem_0);
        let char_1 = b'a'.saturating_add(rem_1);
        let char_2 = b'a'.saturating_add(rem_2);
        let s_chars = [char_0, char_1, char_2];
        let text_str = match std::str::from_utf8(&s_chars) {
            Ok(valid) => valid.to_owned(),
            Err(_) => "abc".to_owned(),
        };
        let bytes_vec = stream.next_u64().to_be_bytes().to_vec();
        let flag = (stream.next_u64() & 1) != 0;
        let id_val = draw_uuid(&mut stream);

        let compound = vec![
            Element::Int(val_int),
            Element::String(Cow::Borrowed(&text_str)),
            Element::Bytes(Cow::Borrowed(&bytes_vec)),
            Element::Bool(flag),
            Element::Uuid(id_val),
        ];

        let mut packed = Vec::new();
        pack_elements(&compound, &mut packed)?;
        let unpacked = unpack_elements(&packed)?;
        assert_eq!(unpacked, compound, "round-trip identity failure");

        elements_list.push((val_int, text_str, bytes_vec, flag, *id_val.as_bytes()));
        packed_list.push(packed);
    }

    for pair_idx in 0..count.saturating_sub(1) {
        let Some(elem_a) = elements_list.get(pair_idx) else {
            break;
        };
        let Some(elem_b) = elements_list.get(pair_idx.saturating_add(1)) else {
            break;
        };
        let Some(packed_a) = packed_list.get(pair_idx) else {
            break;
        };
        let Some(packed_b) = packed_list.get(pair_idx.saturating_add(1)) else {
            break;
        };

        assert_eq!(
            packed_a.cmp(packed_b),
            elem_a.cmp(elem_b),
            "order violation at pair {pair_idx}"
        );
    }

    Ok(())
}
