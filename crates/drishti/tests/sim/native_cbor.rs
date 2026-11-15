//! CBOR and attestation-object sims: the reader is fed trees written by an
//! encoder kept in this file, every strict prefix, hostile heads, bit flips
//! and garbage, and each defect must come back as its own typed refusal.

use std::collections::BTreeMap;

use drishti::native_host::attest::{AppAttestAttestation, AppAttestError};
use drishti::native_host::cbor::{CborValue, MAX_ATTEST_BYTES, decode_attestation};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// Head bytes for a major type and argument, in the shortest form.
fn head(major: u8, value: u64) -> Vec<u8> {
    let base = major.wrapping_shl(5);
    let bytes = value.to_be_bytes();
    if value < 24 {
        vec![base | bytes[7]]
    } else if value <= 0xff {
        vec![base | 24, bytes[7]]
    } else if value <= 0xffff {
        vec![base | 25, bytes[6], bytes[7]]
    } else if value <= 0xffff_ffff {
        vec![base | 26, bytes[4], bytes[5], bytes[6], bytes[7]]
    } else {
        let mut out = vec![base | 27];
        out.extend_from_slice(&bytes);
        out
    }
}

/// Head bytes for a length.
fn length_head(major: u8, length: usize) -> Result<Vec<u8>, String> {
    Ok(head(major, u64::try_from(length).map_err(describe)?))
}

/// CBOR bytes of a value, written independently of the crate.
fn encode(value: &CborValue) -> Result<Vec<u8>, String> {
    match *value {
        CborValue::Unsigned(number) => Ok(head(0, number)),
        CborValue::ByteString(ref bytes) => {
            let mut out = length_head(2, bytes.len())?;
            out.extend_from_slice(bytes);
            Ok(out)
        }
        CborValue::TextString(ref text) => {
            let mut out = length_head(3, text.len())?;
            out.extend_from_slice(text.as_bytes());
            Ok(out)
        }
        CborValue::Array(ref items) => {
            let mut out = length_head(4, items.len())?;
            for item in items {
                out.extend(encode(item)?);
            }
            Ok(out)
        }
        CborValue::Map(ref map) => {
            let mut out = length_head(5, map.len())?;
            for entry in map {
                out.extend(encode(&CborValue::TextString(entry.0.clone()))?);
                out.extend(encode(entry.1)?);
            }
            Ok(out)
        }
        _ => Err(String::from("a value the encoder does not know")),
    }
}

/// A seeded byte.
fn byte(random: &mut SimRng) -> u8 {
    random.next_u64().to_le_bytes()[0]
}

/// Seeded bytes of up to `most` bytes.
fn bytes_up_to(random: &mut SimRng, most: usize) -> Vec<u8> {
    let count = random.below_len(most.saturating_add(1));
    (0..count).map(|_| byte(random)).collect()
}

/// Seeded value tree no deeper than three levels.
fn gen_cbor(random: &mut SimRng, depth: usize) -> CborValue {
    const NUMBERS: &[u64] = &[
        0,
        1,
        23,
        24,
        255,
        256,
        65_535,
        65_536,
        4_294_967_295,
        4_294_967_296,
        u64::MAX,
    ];
    let kinds = if depth >= 3 { 3 } else { 5 };
    match random.below(kinds) {
        0 => CborValue::Unsigned(NUMBERS[random.below_len(NUMBERS.len())]),
        1 => CborValue::ByteString(bytes_up_to(random, 30)),
        2 => {
            let length = random.below_len(10);
            CborValue::TextString(format!("{}\u{e9}", random.text(length)))
        }
        3 => {
            let count = random.below_len(4);
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(gen_cbor(random, depth.saturating_add(1)));
            }
            CborValue::Array(items)
        }
        _ => {
            let count = random.below_len(4);
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let length = random.below_len(6);
                let key = random.text(length);
                map.insert(key, gen_cbor(random, depth.saturating_add(1)));
            }
            CborValue::Map(map)
        }
    }
}

/// Bug caught: a reader that mis-sizes a head (1, 2, 4 or 8 byte arguments)
/// or loses a map key, so a signed object decodes to a different object.
#[test]
fn sim_cbor_roundtrips_seeded_trees() -> Result<(), String> {
    for seed in 0..400_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(130_000));
        let value = gen_cbor(&mut random, 0);
        let bytes = encode(&value)?;
        ensure(
            bytes.len() <= MAX_ATTEST_BYTES,
            "the corpus must stay under the size cap",
        )?;
        ensure(
            decode_attestation(&bytes) == Ok(value.clone()),
            &format!("seed {seed}: {value:?} did not survive {bytes:02x?}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a truncated object that decodes as a shorter one, so a cut
/// transport yields a half-trusted attestation.
#[test]
fn sim_cbor_every_strict_prefix_is_truncated() -> Result<(), String> {
    for seed in 0..60_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(140_000));
        let value = gen_cbor(&mut random, 0);
        let bytes = encode(&value)?;
        for cut in 0..bytes.len() {
            let outcome = decode_attestation(&bytes[..cut]);
            ensure(
                outcome == Err(AppAttestError::Truncated),
                &format!(
                    "seed {seed}: {cut} of {} bytes gave {outcome:?}",
                    bytes.len()
                ),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a reader that panics or loops on garbage, or yields a value
/// that does not survive being written and read again (a lossy decode).
#[test]
fn sim_cbor_garbage_and_bit_flips_never_panic_and_reread() -> Result<(), String> {
    let mut accepted = 0_usize;
    for seed in 0..3_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(150_000));
        let bytes = if seed.wrapping_rem(2) == 0 {
            bytes_up_to(&mut random, 40)
        } else {
            let mut good = encode(&gen_cbor(&mut random, 0))?;
            let flips = random.below_len(3).saturating_add(1);
            for _ in 0..flips {
                let position = random.below_len(good.len());
                if let Some(slot) = good.get_mut(position) {
                    *slot ^= 1 << random.below_len(8);
                }
            }
            good
        };
        if let Ok(value) = decode_attestation(&bytes) {
            accepted = accepted.saturating_add(1);
            ensure(
                decode_attestation(&encode(&value)?) == Ok(value.clone()),
                &format!("{bytes:02x?} decoded to {value:?} which does not re-read"),
            )?;
        }
    }
    ensure(accepted > 100, "the corpus must hold accepted inputs")
}

/// Bug caught: an unsupported head silently treated as a supported one, or
/// an untyped failure, so a hostile object is blamed on the wrong layer.
#[test]
fn sim_cbor_hostile_heads_are_typed_refusals() -> Result<(), String> {
    let table: Vec<(Vec<u8>, AppAttestError)> = vec![
        (vec![0x20], AppAttestError::UnsupportedMajorType(1)),
        (vec![0xc0], AppAttestError::UnsupportedMajorType(6)),
        (vec![0xe0], AppAttestError::UnsupportedMajorType(7)),
        (vec![0xf4], AppAttestError::UnsupportedMajorType(7)),
        (vec![0xf6], AppAttestError::UnsupportedMajorType(7)),
        (vec![0x1c], AppAttestError::UnsupportedAdditionalInfo(28)),
        (vec![0x1f], AppAttestError::UnsupportedAdditionalInfo(31)),
        (vec![0x5f], AppAttestError::UnsupportedAdditionalInfo(31)),
        (vec![0x9f], AppAttestError::UnsupportedAdditionalInfo(31)),
        (vec![0xbf], AppAttestError::UnsupportedAdditionalInfo(31)),
        (
            vec![0x9a, 0x00, 0x10, 0x00, 0x00],
            AppAttestError::Truncated,
        ),
        (
            vec![0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            AppAttestError::Truncated,
        ),
        (
            vec![0xbb, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            AppAttestError::Truncated,
        ),
        (
            vec![0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
            AppAttestError::Truncated,
        ),
        (
            vec![0x7a, 0xff, 0xff, 0xff, 0xff, 0x61],
            AppAttestError::Truncated,
        ),
        (
            vec![0xa1, 0x01, 0x00],
            AppAttestError::UnexpectedType(String::from("map key")),
        ),
        (
            vec![0xa1, 0x41, 0x61, 0x00],
            AppAttestError::UnexpectedType(String::from("map key")),
        ),
        (
            vec![0xa1, 0x80, 0x00],
            AppAttestError::UnexpectedType(String::from("map key")),
        ),
    ];
    for (bytes, want) in table {
        let outcome = decode_attestation(&bytes);
        ensure(
            outcome == Err(want.clone()),
            &format!("{bytes:02x?}: got {outcome:?}, want {want:?}"),
        )?;
    }
    for bad in [
        vec![0x62, 0xff, 0xfe],
        vec![0x61, 0x80],
        vec![0x62, 0xc0, 0x00],
    ] {
        ensure(
            matches!(
                decode_attestation(&bad),
                Err(AppAttestError::InvalidUtf8(_))
            ),
            &format!("{bad:02x?} must be refused as invalid UTF-8"),
        )?;
    }
    let simple_ok = |lead: u8| lead < 24 || matches!(lead, 0x40 | 0x60 | 0x80 | 0xa0);
    for lead in 0..=255_u8 {
        let outcome = decode_attestation(&[lead]);
        ensure(
            outcome.is_ok() == simple_ok(lead),
            &format!("the single byte {lead:#04x} gave {outcome:?}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a depth or size bound off by one, or a recursion that is
/// bounded only by the stack, so a 64 KiB object can crash the verifier.
#[test]
fn sim_cbor_depth_and_size_bounds_are_exact() -> Result<(), String> {
    for levels in 120..=140_usize {
        let mut arrays = vec![0x81_u8; levels];
        arrays.push(0x00);
        let mut maps = Vec::new();
        for _ in 0..levels {
            maps.extend_from_slice(&[0xa1, 0x61, 0x61]);
        }
        maps.push(0x00);
        for (shape, bytes) in [("array", arrays), ("map", maps)] {
            let outcome = decode_attestation(&bytes);
            ensure(
                outcome.is_ok() == (levels <= 128),
                &format!(
                    "{levels} nested {shape}s: verdict {} breaks the bound of 128",
                    outcome.is_ok()
                ),
            )?;
        }
    }
    for levels in [10_000_usize, 21_000, 60_000] {
        let mut arrays = vec![0x81_u8; levels];
        arrays.push(0x00);
        ensure(
            decode_attestation(&arrays) == Err(AppAttestError::Truncated),
            &format!("{levels} nested arrays must be refused, not recursed"),
        )?;
    }
    let payload = MAX_ATTEST_BYTES.saturating_sub(5);
    let mut exact = vec![0x5a];
    exact.extend_from_slice(&u32::try_from(payload).map_err(describe)?.to_be_bytes());
    exact.extend(vec![0_u8; payload]);
    ensure(
        exact.len() == MAX_ATTEST_BYTES,
        "the fixture must fill the cap",
    )?;
    ensure(
        decode_attestation(&exact).is_ok(),
        "an object of exactly the cap must decode",
    )?;
    exact.push(0);
    ensure(
        decode_attestation(&exact) == Err(AppAttestError::Truncated),
        "an object one byte over the cap must be refused",
    )
}

/// Bug caught: a repeated map key where the last silently wins, or bytes
/// after the object that the signer never covered, which smuggle a second
/// reading past a verifier of the first.
#[test]
fn sim_cbor_duplicate_keys_and_trailing_bytes_are_refused() -> Result<(), String> {
    ensure(
        decode_attestation(&[0xa2, 0x61, 0x61, 0x00, 0x61, 0x61, 0x01]).is_err(),
        "a map naming `a` twice must be refused",
    )?;
    ensure(
        decode_attestation(&[0xa2, 0x61, 0x61, 0x00, 0x61, 0x62, 0x01]).is_ok(),
        "a map with two distinct keys must decode",
    )?;
    for seed in 0..120_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(160_000));
        let value = gen_cbor(&mut random, 0);
        let mut bytes = encode(&value)?;
        ensure(
            decode_attestation(&bytes) == Ok(value),
            "the clean object must decode",
        )?;
        let extra = random.below_len(4).saturating_add(1);
        for _ in 0..extra {
            bytes.push(byte(&mut random));
        }
        ensure(
            decode_attestation(&bytes).is_err(),
            &format!("seed {seed}: {extra} trailing bytes were accepted"),
        )?;
    }
    Ok(())
}

/// A text key with its value, for building maps.
fn entry(map: &mut BTreeMap<String, CborValue>, key: &str, value: Option<CborValue>) {
    if let Some(present) = value {
        map.insert(String::from(key), present);
    }
}

/// An attestation object from optional parts; `None` leaves the key out.
fn object(stmt: Option<CborValue>, auth: Option<CborValue>) -> Result<Vec<u8>, String> {
    let mut top = BTreeMap::new();
    entry(
        &mut top,
        "fmt",
        Some(CborValue::TextString(String::from("apple-appattest"))),
    );
    entry(&mut top, "attStmt", stmt);
    entry(&mut top, "authData", auth);
    encode(&CborValue::Map(top))
}

/// An `attStmt` map from optional parts.
fn statement(chain: Option<CborValue>, receipt: Option<CborValue>) -> CborValue {
    let mut stmt = BTreeMap::new();
    entry(&mut stmt, "x5c", chain);
    entry(&mut stmt, "receipt", receipt);
    CborValue::Map(stmt)
}

/// A byte string value.
fn blob(bytes: &[u8]) -> CborValue {
    CborValue::ByteString(bytes.to_vec())
}

/// Bug caught: a malformed attestation object that parses into a partial
/// result, or is blamed on the wrong key, or whose defects are reported in
/// an order that hides the first one.
#[test]
fn sim_attest_object_refusals_are_total_and_ordered() -> Result<(), String> {
    let chain = CborValue::Array(vec![blob(&[1, 2]), blob(&[3])]);
    let auth = blob(&[9, 9]);
    let missing = |key: &str| AppAttestError::MissingKey(String::from(key));
    let wrong = |what: &str| AppAttestError::UnexpectedType(String::from(what));
    let text = || CborValue::TextString(String::from("x"));
    /// One labelled object and the refusal it must draw.
    type Case = (&'static str, Result<Vec<u8>, String>, AppAttestError);
    let cases: Vec<Case> = vec![
        (
            "no attStmt",
            object(None, Some(auth.clone())),
            missing("attStmt"),
        ),
        (
            "attStmt an array",
            object(Some(CborValue::Array(Vec::new())), Some(auth.clone())),
            missing("attStmt"),
        ),
        (
            "no x5c",
            object(Some(statement(None, None)), Some(auth.clone())),
            missing("x5c"),
        ),
        (
            "x5c a map",
            object(
                Some(statement(Some(CborValue::Map(BTreeMap::new())), None)),
                Some(auth.clone()),
            ),
            missing("x5c"),
        ),
        (
            "x5c entry text",
            object(
                Some(statement(
                    Some(CborValue::Array(vec![blob(&[1]), text()])),
                    None,
                )),
                Some(auth.clone()),
            ),
            wrong("x5c entry"),
        ),
        (
            "x5c entry number",
            object(
                Some(statement(
                    Some(CborValue::Array(vec![CborValue::Unsigned(4)])),
                    None,
                )),
                Some(auth.clone()),
            ),
            wrong("x5c entry"),
        ),
        (
            "x5c empty",
            object(
                Some(statement(Some(CborValue::Array(Vec::new())), None)),
                Some(auth),
            ),
            AppAttestError::EmptyCertChain,
        ),
        (
            "no authData",
            object(Some(statement(Some(chain.clone()), None)), None),
            missing("authData"),
        ),
        (
            "authData text",
            object(Some(statement(Some(chain), None)), Some(text())),
            missing("authData"),
        ),
        (
            "empty chain and no authData: the chain is judged first",
            object(
                Some(statement(Some(CborValue::Array(Vec::new())), None)),
                None,
            ),
            AppAttestError::EmptyCertChain,
        ),
        (
            "no attStmt and no authData: the statement is judged first",
            object(None, None),
            missing("attStmt"),
        ),
        (
            "root is an array",
            encode(&CborValue::Array(vec![blob(&[1])])),
            AppAttestError::NotAMap,
        ),
        (
            "root is a number",
            encode(&CborValue::Unsigned(7)),
            AppAttestError::NotAMap,
        ),
    ];
    for (label, bytes, want) in cases {
        let outcome = AppAttestAttestation::parse(&bytes?);
        ensure(
            outcome == Err(want.clone()),
            &format!("{label}: got {outcome:?}, want {want:?}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a parsed attestation that reorders the chain, mislabels the
/// leaf, or drops the receipt, so the verifier checks the wrong certificate.
#[test]
fn sim_attest_object_fields_survive_parsing() -> Result<(), String> {
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(170_000));
        let count = random.below_len(4).saturating_add(1);
        let chain: Vec<Vec<u8>> = (0..count).map(|_| bytes_up_to(&mut random, 40)).collect();
        let auth = bytes_up_to(&mut random, 40);
        let receipt = if random.below(2) == 0 {
            None
        } else {
            Some(bytes_up_to(&mut random, 40))
        };
        let items = CborValue::Array(chain.iter().map(|der| blob(der)).collect());
        let bytes = object(
            Some(statement(Some(items), receipt.as_deref().map(blob))),
            Some(blob(&auth)),
        )?;
        let parsed = AppAttestAttestation::parse(&bytes).map_err(describe)?;
        ensure(
            parsed.chain() == chain.as_slice()
                && parsed.leaf_cert_der() == chain[0].as_slice()
                && parsed.intermediate_certs_der() == &chain[1..]
                && parsed.auth_data() == auth.as_slice()
                && parsed.receipt() == receipt.as_ref(),
            &format!("seed {seed}: parsed fields drifted from the object"),
        )?;
    }
    Ok(())
}

/// Bug caught: an optional `receipt` of the wrong type read as absent, so a
/// malformed (or substituted) receipt passes as a plain attestation instead
/// of failing closed.
#[test]
fn sim_attest_object_refuses_a_mistyped_receipt() -> Result<(), String> {
    let chain = CborValue::Array(vec![blob(&[1, 2])]);
    let wrong = [
        CborValue::TextString(String::from("receipt")),
        CborValue::Unsigned(7),
        CborValue::Array(vec![blob(&[1])]),
        CborValue::Array(Vec::new()),
        CborValue::Map(BTreeMap::new()),
    ];
    for bad in wrong {
        let bytes = object(
            Some(statement(Some(chain.clone()), Some(bad.clone()))),
            Some(blob(&[9])),
        )?;
        ensure(
            AppAttestAttestation::parse(&bytes)
                == Err(AppAttestError::UnexpectedType(String::from("receipt"))),
            &format!("a receipt of {bad:?} must be refused as mistyped"),
        )?;
        let no_auth = object(
            Some(statement(Some(chain.clone()), Some(bad.clone()))),
            None,
        )?;
        ensure(
            AppAttestAttestation::parse(&no_auth)
                == Err(AppAttestError::MissingKey(String::from("authData"))),
            "the first defect in reading order is still the one reported",
        )?;
        let no_chain = object(
            Some(statement(Some(CborValue::Array(Vec::new())), Some(bad))),
            Some(blob(&[9])),
        )?;
        ensure(
            AppAttestAttestation::parse(&no_chain) == Err(AppAttestError::EmptyCertChain),
            "an empty chain is reported before the receipt",
        )?;
    }
    let clean = object(
        Some(statement(Some(chain.clone()), Some(blob(&[4, 5])))),
        Some(blob(&[9])),
    )?;
    let parsed = AppAttestAttestation::parse(&clean).map_err(describe)?;
    ensure(
        parsed.receipt() == Some(&vec![4, 5]),
        "a byte-string receipt is kept",
    )?;
    let absent = object(Some(statement(Some(chain), None)), Some(blob(&[9])))?;
    ensure(
        AppAttestAttestation::parse(&absent)
            .map_err(describe)?
            .receipt()
            .is_none(),
        "an absent receipt stays absent",
    )
}
