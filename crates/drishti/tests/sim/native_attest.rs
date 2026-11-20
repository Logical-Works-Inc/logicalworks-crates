//! Attestation-evidence sims: receipts, the DER writer, the TrustZone quote
//! and the key checks are fed liar-upstream input (wrong lengths, bad tags,
//! off-curve points) and compared with a DER reader and a verifier kept in
//! this file; every refusal is typed and total.

use drishti::native_host::attest::{
    AppAttestProductionError, AttestError, BootChallengeReceipt, app_attest_is_supported, apple,
    der, produce_app_attest_evidence, trustzone,
};
use lgwks_deps::p256;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// Seeded bytes of an exact length.
fn exact_bytes(random: &mut SimRng, length: usize) -> Vec<u8> {
    (0..length)
        .map(|_| {
            let [low, ..] = random.next_u64().to_le_bytes();
            low
        })
        .collect()
}

/// A deterministic device key from the seed.
fn device_key(random: &mut SimRng) -> Result<SigningKey, String> {
    let mut secret = exact_bytes(random, 32);
    let first = secret.first_mut().ok_or("the secret has no first byte")?;
    *first = 0;
    let last = secret.last_mut().ok_or("the secret has no last byte")?;
    *last |= 1;
    SigningKey::from_slice(&secret).map_err(describe)
}

/// A well-formed receipt over seeded challenge and MAC.
fn receipt(random: &mut SimRng) -> Result<BootChallengeReceipt, String> {
    let issued = exact_bytes(random, 32);
    let consumed = exact_bytes(random, 32);
    let mac = exact_bytes(random, 32);
    BootChallengeReceipt::new(&issued, 10, 20, &consumed, 15, &mac).map_err(describe)
}

/// The compressed SEC1 form of a key's public point.
fn compressed(key: &SigningKey) -> Vec<u8> {
    VerifyingKey::from(key)
        .to_sec1_point(true)
        .as_bytes()
        .to_vec()
}

/// Bug caught: a receipt built from a challenge, consumption or MAC of the
/// wrong size (a short one is zero-padded or truncated somewhere), which
/// would bind issuance to the wrong challenge.
#[test]
fn sim_attest_receipt_demands_32_bytes_everywhere() -> Result<(), String> {
    let sizes = [0_usize, 1, 31, 32, 33, 64];
    for issued_size in sizes {
        for consumed_size in sizes {
            for mac_size in sizes {
                let mut random = SimRng::seed(
                    u64::try_from(
                        issued_size
                            .saturating_add(consumed_size)
                            .saturating_add(mac_size),
                    )
                    .map_err(describe)?,
                );
                let issued = exact_bytes(&mut random, issued_size);
                let consumed = exact_bytes(&mut random, consumed_size);
                let mac = exact_bytes(&mut random, mac_size);
                let built = BootChallengeReceipt::new(&issued, 1, 2, &consumed, 3, &mac);
                let want = issued_size == 32 && consumed_size == 32 && mac_size == 32;
                ensure(
                    built.is_ok() == want,
                    &format!(
                        "sizes ({issued_size}, {consumed_size}, {mac_size}): verdict {} but the rule says {want}",
                        built.is_ok()
                    ),
                )?;
                match built {
                    Ok(held) => ensure(
                        held.issued_challenge().as_slice() == issued.as_slice()
                            && held.consumed_challenge().as_slice() == consumed.as_slice()
                            && held.issue_mac().as_slice() == mac.as_slice()
                            && held.issued_at_epoch() == 1
                            && held.expires_at_epoch() == 2
                            && held.consumed_at_epoch() == 3,
                        "a receipt must carry its inputs unchanged",
                    )?,
                    Err(error) => ensure(
                        error == AttestError::ChallengeMustBe32Bytes,
                        "a wrong size must be the typed size refusal",
                    )?,
                }
            }
        }
    }
    Ok(())
}

/// One parsed DER element.
struct Tlv<'a> {
    /// Tag byte.
    tag: u8,
    /// Content bytes.
    content: &'a [u8],
    /// Bytes after the element.
    rest: &'a [u8],
}

/// Strict DER reader: definite minimal lengths only.
fn read_tlv(bytes: &[u8]) -> Result<Tlv<'_>, String> {
    let tag = *bytes.first().ok_or("no tag")?;
    let lead = *bytes.get(1).ok_or("no length")?;
    let (length, header) = if lead < 0x80 {
        (usize::from(lead), 2_usize)
    } else {
        let count = usize::from(lead & 0x7f);
        ensure(count != 0 && count <= 4, "unsupported length form")?;
        let digits = bytes
            .get(2..2_usize.saturating_add(count))
            .ok_or("short length")?;
        ensure(
            digits.first().is_some_and(|digit| *digit != 0),
            "non-minimal length",
        )?;
        let mut value = 0_usize;
        for digit in digits {
            value = value.wrapping_shl(8) | usize::from(*digit);
        }
        ensure(value >= 0x80, "long form for a short length")?;
        (value, 2_usize.saturating_add(count))
    };
    let end = header.saturating_add(length);
    let content = bytes.get(header..end).ok_or("content past the end")?;
    let rest = bytes.get(end..).ok_or("rest past the end")?;
    Ok(Tlv { tag, content, rest })
}

/// Bug caught: a DER length written in a non-minimal or wrong form around
/// the 127/128, 255/256 and 65535/65536 byte edges, or an integer whose high
/// bit turns a small positive number negative.
#[test]
fn sim_attest_der_writer_agrees_with_a_strict_reader() -> Result<(), String> {
    for length in [
        0_usize, 1, 2, 126, 127, 128, 129, 255, 256, 257, 4_096, 65_535, 65_536, 70_000,
    ] {
        let content = vec![0xab_u8; length];
        for (tag, written) in [
            (0x04_u8, der::octet_string(&content)),
            (0x0c_u8, der::utf8_string(&"a".repeat(length))),
        ] {
            let parsed = read_tlv(&written)?;
            ensure(
                parsed.tag == tag && parsed.content.len() == length && parsed.rest.is_empty(),
                &format!("length {length}: tag {tag:#04x} did not read back"),
            )?;
        }
        let wrapped = der::sequence(&[der::octet_string(&content), der::integer(1)]);
        let outer = read_tlv(&wrapped)?;
        let first = read_tlv(outer.content)?;
        let second = read_tlv(first.rest)?;
        ensure(
            outer.tag == 0x30
                && first.tag == 0x04
                && first.content.len() == length
                && second.tag == 0x02
                && second.content == [1]
                && second.rest.is_empty(),
            &format!("length {length}: a sequence did not nest back"),
        )?;
    }
    for value in 0..=255_u8 {
        let written = der::integer(value);
        let parsed = read_tlv(&written)?;
        let negative = parsed.content.first().is_some_and(|lead| *lead >= 0x80);
        let magnitude = parsed
            .content
            .iter()
            .fold(0_u32, |sum, digit| sum.wrapping_shl(8) | u32::from(*digit));
        ensure(
            parsed.tag == 0x02 && !negative && magnitude == u32::from(value),
            &format!("integer {value}: read back as {magnitude} (negative: {negative})"),
        )?;
    }
    Ok(())
}

/// Bug caught: a quote whose signature does not cover its body, whose fields
/// land in the wrong slots, or that accepts a challenge of the wrong size.
#[test]
fn sim_attest_quote_is_signed_over_its_body_and_checks_the_challenge() -> Result<(), String> {
    for seed in 0..40_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(180_000));
        let key = device_key(&mut random)?;
        let verifier = VerifyingKey::from(&key);
        let oem_tail = random.below_len(200);
        let oem = format!("oem-\u{e9}-{}", random.text(oem_tail));
        let challenge = exact_bytes(&mut random, 32);
        let spki_length = random.below_len(150);
        let spki = exact_bytes(&mut random, spki_length);
        let quote = trustzone::make_quote(&oem, &challenge, &spki, &key).map_err(describe)?;
        let outer = read_tlv(&quote)?;
        let body_element = read_tlv(outer.content)?;
        let signature_element = read_tlv(body_element.rest)?;
        ensure(
            outer.tag == 0x30
                && body_element.tag == 0x04
                && signature_element.tag == 0x04
                && signature_element.rest.is_empty(),
            &format!("seed {seed}: the quote is not SEQ {{ OCTET, OCTET }}"),
        )?;
        let body = read_tlv(body_element.content)?;
        let version = read_tlv(body.content)?;
        let name = read_tlv(version.rest)?;
        let quoted_challenge = read_tlv(name.rest)?;
        let quoted_key = read_tlv(quoted_challenge.rest)?;
        ensure(
            body.tag == 0x30
                && version.content == [1]
                && name.tag == 0x0c
                && name.content == oem.as_bytes()
                && quoted_challenge.content == challenge.as_slice()
                && quoted_key.content == spki.as_slice()
                && quoted_key.rest.is_empty(),
            &format!("seed {seed}: a body field is in the wrong slot"),
        )?;
        let signature = Signature::from_der(signature_element.content).map_err(describe)?;
        ensure(
            verifier.verify(body_element.content, &signature).is_ok(),
            &format!("seed {seed}: the signature does not cover the body"),
        )?;
        let mut tampered = body_element.content.to_vec();
        let position = random.below_len(tampered.len());
        let flipped = tampered.get_mut(position).ok_or("the body is empty")?;
        *flipped ^= 0x01;
        ensure(
            verifier.verify(&tampered, &signature).is_err(),
            &format!("seed {seed}: a flipped body byte still verified"),
        )?;
        for size in [0_usize, 1, 31, 33, 64] {
            let outcome = trustzone::make_quote(&oem, &exact_bytes(&mut random, size), &spki, &key);
            ensure(
                outcome == Err(AttestError::ChallengeMustBe32Bytes),
                &format!("seed {seed}: a {size}-byte challenge gave {outcome:?}"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: an attested key accepted because it has 33 bytes and a `02`
/// or `03` tag, though its x coordinate is on no curve point, so evidence can
/// name a key no device can hold.
#[test]
fn sim_attest_attested_key_must_be_a_real_curve_point() -> Result<(), String> {
    let mut random = SimRng::seed(190_000);
    let key = device_key(&mut random)?;
    let held = receipt(&mut random)?;
    let sizes = [0_usize, 1, 32, 33, 34, 65];
    let tags = [0_u8, 1, 2, 3, 4, 0xff];
    let mut accepted = 0_usize;
    let mut off_curve = 0_usize;
    for attempt in 0..40_usize {
        for size in sizes {
            for tag in tags {
                let mut candidate = exact_bytes(&mut random, size);
                if let Some(lead) = candidate.first_mut() {
                    *lead = tag;
                }
                if attempt == 0 && size == 33 && (tag == 2 || tag == 3) {
                    candidate = compressed(&key);
                }
                let shaped =
                    candidate.len() == 33 && matches!(candidate.first().copied(), Some(2 | 3));
                let want = shaped && VerifyingKey::from_sec1_bytes(&candidate).is_ok();
                let by_apple = apple::make_package(vec![1], Vec::new(), &held, &candidate, &key);
                let by_arm =
                    trustzone::make_package("oem", &held, &[1, 2, 3], &candidate, Vec::new(), &key);
                ensure(
                    by_apple.is_ok() == want && by_arm.is_ok() == want,
                    &format!(
                        "key {candidate:02x?}: apple {} arm {} but the rule says {want}",
                        by_apple.is_ok(),
                        by_arm.is_ok()
                    ),
                )?;
                if want {
                    accepted = accepted.saturating_add(1);
                } else if shaped {
                    off_curve = off_curve.saturating_add(1);
                    ensure(
                        by_apple.err() == Some(AttestError::AttestedKeyMustBeCompressedP256Sec1)
                            && by_arm.err()
                                == Some(AttestError::AttestedKeyMustBeCompressedP256Sec1),
                        "an off-curve key must be the typed key refusal",
                    )?;
                }
            }
        }
    }
    ensure(
        accepted > 3 && off_curve > 3,
        "the corpus must hold accepted and off-curve keys",
    )
}

/// Bug caught: an Apple package whose proof-of-possession signs something
/// other than the issued challenge, or that carries another receipt's
/// challenge, so evidence for one boot is replayed for another.
#[test]
fn sim_attest_apple_package_proves_possession_of_the_issued_challenge() -> Result<(), String> {
    for seed in 0..40_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(200_000));
        let key = device_key(&mut random)?;
        let held = receipt(&mut random)?;
        let attested = compressed(&key);
        let package = apple::make_package(vec![7, 7], vec![vec![8]], &held, &attested, &key)
            .map_err(describe)?;
        let signature =
            Signature::from_der(package.challenge_pop_signature_der()).map_err(describe)?;
        let verifier = VerifyingKey::from(&key);
        ensure(
            package.challenge() == held.issued_challenge()
                && package.challenge_receipt() == &held
                && package.attested_key_compressed_sec1().as_slice() == attested.as_slice()
                && package.key_cert_der() == [7, 7]
                && package.intermediate_certs_der() == [vec![8_u8]],
            &format!("seed {seed}: the package altered its inputs"),
        )?;
        ensure(
            verifier
                .verify(&held.issued_challenge(), &signature)
                .is_ok(),
            &format!("seed {seed}: the signature does not cover the issued challenge"),
        )?;
        ensure(
            verifier
                .verify(&held.consumed_challenge(), &signature)
                .is_err(),
            &format!("seed {seed}: the signature also verifies the consumed challenge"),
        )?;
    }
    Ok(())
}

/// Bug caught: an on-device production path that soft-falls back to a lower
/// hardware tier, or reports support it does not have.
#[test]
fn sim_attest_production_fails_closed_off_device() -> Result<(), String> {
    ensure(!app_attest_is_supported(), "no entitlement exists here")?;
    for seed in 0..50_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(210_000));
        let held = receipt(&mut random)?;
        ensure(
            produce_app_attest_evidence(&held)
                == Err(AppAttestProductionError::AppAttestUnavailable),
            &format!("seed {seed}: production did not fail closed"),
        )?;
    }
    Ok(())
}
