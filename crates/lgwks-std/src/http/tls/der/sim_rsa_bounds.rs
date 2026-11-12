//! Seeded simulations of the RSA modulus bound on both sides of the proof.
//!
//! Source: #913 item 3 (every RSA-4096 certificate and key was refused: a
//! 4,096-bit modulus is a 513-byte DER INTEGER and the bound was 512), ITU-T
//! X.690 section 8.3 (an INTEGER's content is minimal two's complement, so a
//! magnitude with its top bit set carries one leading zero byte) and RFC 8017
//! appendix A.1 (`RSAPublicKey`, `RSAPrivateKey`). The encoder below is
//! written from X.690, not from the reader under test. One seed draws a
//! modulus width and its bytes; the reader must admit every width up to
//! 8,192 bits on the key side and the certificate side, and refuse one byte
//! more.
//!
//! The DER walk moved here whole from `tls.rs`, so its shape checks are held
//! too: RFC 5280 section 4.1.1.2 (a signature AlgorithmIdentifier opens with
//! an OID), RFC 5915 section 3 (a SEC1 key's `[0]` parameters are one curve
//! OID) and RFC 5208 section 5 (a PKCS#8 `privateKey` is an OCTET STRING).
//! Each refusal is asserted by its own reason, so a check that weakens to a
//! neighbouring refusal is caught.

use super::{
    CertifiedPublic, KeyPublic, oid, parse_private_key, read_children, read_tlv, rsa_public,
    signature_oid_of,
};
use crate::seeded::Seeded;

/// What a test returns: any failure, named.
type Outcome = Result<(), Box<dyn std::error::Error>>;

/// Seeds drawn per family.
const SEEDS: u64 = 512;

/// One DER TLV with a minimal definite length (X.690 section 8.1.3).
fn tlv(tag: u8, body: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut out = vec![tag];
    if let Ok(short) = u8::try_from(body.len())
        && short < 0x80
    {
        out.push(short);
        out.extend_from_slice(body);
        return Ok(out);
    }
    let width = u32::try_from(body.len())?.to_be_bytes();
    let significant: Vec<u8> = width
        .iter()
        .copied()
        .skip_while(|byte| *byte == 0)
        .collect();
    out.push(0x80 | u8::try_from(significant.len())?);
    out.extend_from_slice(&significant);
    out.extend_from_slice(body);
    Ok(out)
}

/// A seeded RSA modulus magnitude of exactly `bytes` bytes, top bit set.
fn magnitude(rng: &mut Seeded, bytes: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (0..bytes)
        .map(|_| rng.next_u64().to_le_bytes()[0])
        .collect();
    if let Some(first) = out.first_mut() {
        *first |= 0x80;
    }
    out
}

/// The INTEGER content X.690 gives a positive magnitude with its top bit set.
fn integer_content(magnitude: &[u8]) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend_from_slice(magnitude);
    out
}

/// A PKCS#1 `RSAPrivateKey` carrying `modulus` content, exponent 65537 and a
/// placeholder private exponent (the reader proves the public half only).
fn pkcs1(modulus: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut body = tlv(0x02, &[0x00])?;
    body.extend(tlv(0x02, modulus)?);
    body.extend(tlv(0x02, &[0x01, 0x00, 0x01])?);
    body.extend(tlv(0x02, &[0x01])?);
    tlv(0x30, &body)
}

/// The `RSAPublicKey` sequence a certificate's subject key carries.
fn rsa_public_key(modulus: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut body = tlv(0x02, modulus)?;
    body.extend(tlv(0x02, &[0x01, 0x00, 0x01])?);
    tlv(0x30, &body)
}

/// Read a certificate-side `RSAPublicKey` through the shared reader.
fn certified(der: &[u8]) -> Result<CertifiedPublic, crate::http::Error> {
    let (sequence, _) = read_tlv(der)?;
    let (modulus, exponent) = rsa_public(&read_children(sequence.body)?, 0)?;
    Ok(CertifiedPublic::Rsa { modulus, exponent })
}

/// The modulus a key-side parse returned, or `None` for a curve key.
fn key_modulus(key: &KeyPublic) -> Option<&[u8]> {
    match *key {
        KeyPublic::Rsa { ref modulus, .. } => Some(modulus),
        KeyPublic::Ec { .. } => None,
    }
}

/// The issue's own case: a 4,096-bit modulus (513 content bytes) is admitted
/// as a key and as a certificate subject key.
#[test]
fn an_rsa_4096_key_and_subject_key_are_admitted() -> Outcome {
    let mut rng = Seeded::from_seed(4096);
    let content = integer_content(&magnitude(&mut rng, 512));
    assert_eq!(
        content.len(),
        513,
        "X.690 gives 4,096 bits 513 content bytes"
    );
    let key = parse_private_key(&pkcs1(&content)?)?;
    assert_eq!(
        key_modulus(&key),
        Some(content.as_slice()),
        "the key's modulus"
    );
    assert!(
        matches!(
            certified(&rsa_public_key(&content)?)?,
            CertifiedPublic::Rsa { .. }
        ),
        "the subject key reads as RSA"
    );
    Ok(())
}

/// Every seeded width from 1,024 to 8,192 bits is admitted on both sides;
/// one byte past 8,192 bits is refused on both sides; the same seed draws
/// the same width and bytes.
#[test]
fn sim_every_width_up_to_8192_bits_is_admitted_and_one_byte_more_is_not() -> Outcome {
    for seed in 0..SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let width = usize::try_from(rng.below(897)?)?.saturating_add(128);
        let content = integer_content(&magnitude(&mut rng, width));
        let key = parse_private_key(&pkcs1(&content)?)?;
        assert_eq!(
            key_modulus(&key),
            Some(content.as_slice()),
            "seed {seed}: {width} B"
        );
        certified(&rsa_public_key(&content)?)?;
        let mut replay = Seeded::from_seed(seed);
        let replay_width = usize::try_from(replay.below(897)?)?.saturating_add(128);
        assert_eq!(replay_width, width, "seed {seed}: one seed, one width");
        assert_eq!(
            magnitude(&mut replay, width),
            content.get(1..).ok_or("content")?.to_vec(),
            "seed {seed}: one seed, one modulus"
        );
    }
    let mut rng = Seeded::from_seed(8193);
    let past = integer_content(&magnitude(&mut rng, 1_025));
    assert!(
        parse_private_key(&pkcs1(&past)?).is_err(),
        "8,200 bits is refused as a key"
    );
    assert!(
        certified(&rsa_public_key(&past)?).is_err(),
        "and as a subject key"
    );
    Ok(())
}

/// A 1,025-byte magnitude with its top bit clear has no leading zero, so the
/// content length alone would admit it; the magnitude bound refuses it.
#[test]
fn a_wide_magnitude_without_its_sign_byte_is_still_refused() -> Outcome {
    let mut rng = Seeded::from_seed(7);
    let mut wide = magnitude(&mut rng, 1_025);
    if let Some(first) = wide.first_mut() {
        *first = 0x01;
    }
    assert!(
        parse_private_key(&pkcs1(&wide)?).is_err(),
        "8,193 bits is refused"
    );
    assert!(certified(&rsa_public_key(&wide)?).is_err(), "on both sides");
    Ok(())
}

/// The reason a refusal names, read from its rendering; an admission is `None`.
fn reason<T>(result: Result<T, crate::http::Error>) -> Option<String> {
    result.err().map(|error| format!("{error:?}"))
}

/// The curves a SEC1 key may name, each an admitted OID.
const CURVES: [&[u8]; 3] = [oid::PRIME256V1, oid::SECP384R1, oid::SECP521R1];

/// A SEC1 `ECPrivateKey` (RFC 5915 section 3) carrying `parameters` as its
/// `[0]` field and a seeded public point as its `[1]` field.
fn sec1(parameters: &[u8], rng: &mut Seeded) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut point = vec![0x00, 0x04];
    point.extend(magnitude(rng, 64));
    let mut body = tlv(0x02, &[0x01])?;
    body.extend(tlv(0x04, &magnitude(rng, 32))?);
    body.extend(tlv(0xA0, parameters)?);
    body.extend(tlv(0xA1, &tlv(0x03, &point)?)?);
    tlv(0x30, &body)
}

/// A PKCS#8 `PrivateKeyInfo` (RFC 5208 section 5) for RSA whose third field
/// is `private` exactly as given.
fn pkcs8_rsa(private: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut algorithm = tlv(0x06, oid::RSA_ENCRYPTION)?;
    algorithm.extend([0x05, 0x00]);
    let mut body = tlv(0x02, &[0x00])?;
    body.extend(tlv(0x30, &algorithm)?);
    body.extend_from_slice(private);
    tlv(0x30, &body)
}

/// A signature AlgorithmIdentifier whose first field is not an OID, or that
/// is empty, is refused as "not an OID", never read on as a scheme.
#[test]
fn a_signature_identifier_without_an_oid_is_refused_by_name() -> Outcome {
    for identifier in [tlv(0x30, &tlv(0x02, &[0x01])?)?, tlv(0x30, &[])?] {
        let (sequence, _) = read_tlv(&identifier)?;
        let why = reason(signature_oid_of(&sequence)).ok_or("admitted a non-OID")?;
        assert!(why.contains("signature is not an OID"), "{why}");
    }
    let mut admitted = tlv(0x06, oid::ECDSA_SHA256)?;
    admitted = tlv(0x30, &admitted)?;
    let (sequence, _) = read_tlv(&admitted)?;
    assert_eq!(
        signature_oid_of(&sequence)?,
        oid::ECDSA_SHA256,
        "an OID is read"
    );
    Ok(())
}

/// A SEC1 key naming its own curve in `[0]` is admitted with that curve, for
/// every admitted curve and seeded point; `[0]` holding two fields is refused.
#[test]
fn sim_a_sec1_key_naming_its_curve_is_admitted_with_it() -> Outcome {
    for seed in 0..SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let curve = match rng.below(3)? {
            0 => CURVES[0],
            1 => CURVES[1],
            _ => CURVES[2],
        };
        let key = parse_private_key(&sec1(&tlv(0x06, curve)?, &mut rng)?)?;
        match key {
            KeyPublic::Ec { curve: read, .. } => assert_eq!(read, curve, "seed {seed}"),
            KeyPublic::Rsa { .. } => return Err(format!("seed {seed}: read as RSA").into()),
        }
        let mut two = tlv(0x06, curve)?;
        two.extend(tlv(0x06, curve)?);
        let why = reason(parse_private_key(&sec1(&two, &mut rng)?)).ok_or("admitted two")?;
        assert!(
            why.contains("EC parameters are not a curve"),
            "seed {seed}: {why}"
        );
    }
    Ok(())
}

/// A PKCS#8 key whose private value is not an OCTET STRING is refused, even
/// when the bytes inside would read as a valid PKCS#1 key; an empty OCTET
/// STRING is refused too, and the OCTET STRING form is admitted.
#[test]
fn a_pkcs8_private_value_must_be_a_non_empty_octet_string() -> Outcome {
    let mut rng = Seeded::from_seed(8);
    let inner = pkcs1(&integer_content(&magnitude(&mut rng, 256)))?;
    for private in [tlv(0x30, &inner)?, tlv(0x04, &[])?] {
        let why = reason(parse_private_key(&pkcs8_rsa(&private)?)).ok_or("admitted")?;
        assert!(why.contains("PKCS#8 key has no private value"), "{why}");
    }
    assert!(
        matches!(
            parse_private_key(&pkcs8_rsa(&tlv(0x04, &inner)?)?)?,
            KeyPublic::Rsa { .. }
        ),
        "the OCTET STRING form reads as RSA"
    );
    Ok(())
}
