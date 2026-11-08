//! Simulation and Wycheproof/NIST test suite for `lgwks_std::ec` (#794, #840).

use crate::ec::{
    Curve, EcError, EcdsaSignature, PublicPoint, SecretScalar, ecdh_shared_secret,
    ecdsa_sign_digest, ecdsa_verify_digest,
};
use crate::seeded::Seeded;

fn make_test_secret(curve: Curve, val: u8) -> Result<SecretScalar, EcError> {
    let mut scalar_bytes = vec![0u8; curve.scalar_len()];
    if let Some(last) = scalar_bytes.last_mut() {
        *last = val;
    }
    SecretScalar::from_be_bytes(curve, &scalar_bytes)
}

#[test]
fn nist_base_point_keygens_and_roundtrips_sec1() -> Result<(), EcError> {
    for curve_ref in &[Curve::P256, Curve::P384, Curve::P521] {
        let curve = *curve_ref;
        let secret = make_test_secret(curve, 1)?;
        let pub_key = secret.public_key()?;
        let uncompressed = pub_key.to_sec1_uncompressed();
        let compressed = pub_key.to_sec1_compressed();

        let parsed_uncomp = PublicPoint::from_sec1_bytes(curve, &uncompressed)?;
        let parsed_comp = PublicPoint::from_sec1_bytes(curve, &compressed)?;
        assert_eq!(
            pub_key, parsed_uncomp,
            "uncompressed SEC1 round-trip must preserve point"
        );
        assert_eq!(
            pub_key, parsed_comp,
            "compressed SEC1 round-trip must preserve point"
        );
    }
    Ok(())
}

#[test]
fn sim_ecdsa_sign_verify_and_tamper_refusal() -> Result<(), EcError> {
    for curve_ref in &[Curve::P256, Curve::P384] {
        let curve = *curve_ref;
        let secret = make_test_secret(curve, 7)?;
        let pub_key = secret.public_key()?;
        let digest = b"logicalworks-nist-ecdsa-digest-32";

        let sig = ecdsa_sign_digest(&secret, digest)?;
        ecdsa_verify_digest(&pub_key, digest, &sig)?;

        // DER and raw round-trips
        let der = sig.to_der();
        let from_der = EcdsaSignature::from_der(curve, &der)?;
        assert_eq!(sig, from_der, "DER round-trip must match");

        let raw = sig.to_raw();
        let from_raw = EcdsaSignature::from_raw(curve, &raw)?;
        assert_eq!(sig, from_raw, "raw signature round-trip must match");

        // Tampered digest must fail verification
        let bad_digest = b"logicalworks-nist-ecdsa-digest-00";
        assert_eq!(
            ecdsa_verify_digest(&pub_key, bad_digest, &sig),
            Err(EcError::SignatureInvalid),
            "tampered digest must be refused"
        );
    }
    Ok(())
}

#[test]
fn sim_ecdh_commutativity_and_invalid_point_rejection() -> Result<(), EcError> {
    for curve_ref in &[Curve::P256, Curve::P384, Curve::P521] {
        let curve = *curve_ref;
        let alice_sec = make_test_secret(curve, 3)?;
        let bob_sec = make_test_secret(curve, 5)?;
        let alice_pub = alice_sec.public_key()?;
        let bob_pub = bob_sec.public_key()?;

        let shared_ab = ecdh_shared_secret(&alice_sec, &bob_pub)?;
        let shared_ba = ecdh_shared_secret(&bob_sec, &alice_pub)?;
        assert_eq!(
            shared_ab.as_bytes(),
            shared_ba.as_bytes(),
            "ECDH shared secret must commute"
        );

        // Corrupt the y-coordinate of bob_pub to create an off-curve point
        let mut corrupted = bob_pub.to_sec1_uncompressed();
        if let Some(last) = corrupted.last_mut() {
            *last ^= 0x01;
        }
        assert_eq!(
            PublicPoint::from_sec1_bytes(curve, &corrupted),
            Err(EcError::InvalidPoint),
            "off-curve point must be refused"
        );
    }
    Ok(())
}

#[test]
fn wycheproof_der_and_zero_scalar_refusals() {
    let zero_32 = [0u8; 32];
    assert_eq!(
        SecretScalar::from_be_bytes(Curve::P256, &zero_32),
        Err(EcError::InvalidScalar),
        "zero scalar must be refused"
    );
    // Truncated DER sequence
    assert!(
        EcdsaSignature::from_der(Curve::P256, &[0x30, 0x06, 0x02, 0x01, 0x01]).is_err(),
        "truncated DER must be refused"
    );
    // Non-minimal DER integer (leading 0x00 before < 0x80)
    assert!(
        EcdsaSignature::from_der(
            Curve::P256,
            &[0x30, 0x07, 0x02, 0x02, 0x00, 0x01, 0x02, 0x01, 0x01]
        )
        .is_err(),
        "non-minimal DER integer must be refused"
    );
}

#[test]
fn sim_ec_seeded_sign_verify_sweep() -> Result<(), EcError> {
    let mut stream = Seeded::from_seed(0x5EED_0000_0000_0001);
    for _step in 0..8 {
        let mut scalar_bytes = [0u8; 32];
        for chunk in scalar_bytes.as_chunks_mut::<8>().0 {
            chunk.copy_from_slice(&stream.next_u64().to_le_bytes());
        }
        scalar_bytes[0] &= 0x7f;
        scalar_bytes[31] |= 0x01;
        let secret = SecretScalar::from_be_bytes(Curve::P256, &scalar_bytes)?;
        let pub_key = secret.public_key()?;
        let mut digest = [0u8; 32];
        for chunk in digest.as_chunks_mut::<8>().0 {
            chunk.copy_from_slice(&stream.next_u64().to_le_bytes());
        }
        let sig = ecdsa_sign_digest(&secret, &digest)?;
        ecdsa_verify_digest(&pub_key, &digest, &sig)?;
    }
    Ok(())
}

#[test]
fn sim_ec_weird_bytes_der_malformed_storm() {
    let mut stream = Seeded::from_seed(0x5EED_0000_0000_0002);
    for _step in 0..64 {
        let mut buf = [0u8; 24];
        for chunk in buf.as_chunks_mut::<8>().0 {
            chunk.copy_from_slice(&stream.next_u64().to_le_bytes());
        }
        // Any corrupted header or truncated buffer must never panic
        drop(EcdsaSignature::from_der(Curve::P256, &buf));
        drop(PublicPoint::from_sec1_bytes(Curve::P256, &buf));
    }
}

#[test]
fn sim_ec_saboteur_bitflip_signature_rejection() -> Result<(), EcError> {
    let mut stream = Seeded::from_seed(0x5EED_0000_FFFF_FFFF);
    let mut scalar_bytes = [0u8; 32];
    scalar_bytes[31] = 11;
    let secret = SecretScalar::from_be_bytes(Curve::P256, &scalar_bytes)?;
    let pub_key = secret.public_key()?;
    let digest = b"saboteur-bitflip-target-digest32";
    let sig = ecdsa_sign_digest(&secret, digest)?;
    let raw_orig = sig.to_raw();
    for _step in 0..16 {
        let mut raw_corrupt = raw_orig.clone();
        let mut idx = 0_usize;
        if let Ok(u_val) = usize::try_from(stream.next_u64() & 63) {
            idx = u_val;
        }
        if let Some(byte_ref) = raw_corrupt.get_mut(idx) {
            *byte_ref ^= 0x01;
        }
        if let Ok(corrupt_sig) = EcdsaSignature::from_raw(Curve::P256, &raw_corrupt) {
            assert_eq!(
                ecdsa_verify_digest(&pub_key, digest, &corrupt_sig),
                Err(EcError::SignatureInvalid),
                "bit-flipped signature must be refused"
            );
        }
    }
    Ok(())
}
