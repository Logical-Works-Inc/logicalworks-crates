//! ARM TrustZone quote producer: DER quote shape plus package assembly.

use lgwks_deps::p256;
use p256::ecdsa::signature::Signer as _;

use super::super::ensure;
use super::{
    ArmTrustZoneEvidencePackage, AttestError, BootChallengeReceipt, der, require_compressed_point,
};

/// Build a DER quote over OEM id, challenge, and key SPKI, signed by the
/// device key. Body is `SEQ { INT 1, UTF8 oem, OCTET challenge, OCTET
/// spki }`; the quote wraps body and signature as two octet strings.
///
/// # Errors
///
/// Returns [`AttestError::ChallengeMustBe32Bytes`] for a wrongly sized
/// challenge.
/// # Examples
/// ```
/// use drishti::native_host::attest::{AttestError, trustzone};
/// use lgwks_deps::p256;
/// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
/// let quote = trustzone::make_quote("oem", &[7; 32], &[0x30, 0x03], &key)?;
/// assert_eq!(quote.first(), Some(&0x30));
/// let short = trustzone::make_quote("oem", &[7; 31], &[0x30, 0x03], &key);
/// assert_eq!(short.err(), Some(AttestError::ChallengeMustBe32Bytes));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn make_quote(
    oem_id: &str,
    challenge: &[u8],
    attested_key_spki_der: &[u8],
    signing_key: &p256::ecdsa::SigningKey,
) -> Result<Vec<u8>, AttestError> {
    ensure(challenge.len() == 32, || {
        AttestError::ChallengeMustBe32Bytes
    })?;
    let body = der::sequence(&[
        der::integer(1),
        der::utf8_string(oem_id),
        der::octet_string(challenge),
        der::octet_string(attested_key_spki_der),
    ]);
    let signature: p256::ecdsa::Signature = signing_key.sign(&body);
    Ok(der::sequence(&[
        der::octet_string(&body),
        der::octet_string(signature.to_der().as_bytes()),
    ]))
}

/// Assemble a TrustZone package, validating the attested key shape.
///
/// # Errors
///
/// Returns [`AttestError::AttestedKeyMustBeCompressedP256Sec1`] for a
/// malformed attested key, or the quote error for a bad challenge.
/// # Examples
/// ```
/// use drishti::native_host::attest::{AttestError, BootChallengeReceipt, trustzone};
/// use lgwks_deps::p256;
/// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
/// let point = key.verifying_key().to_sec1_point(true);
/// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
/// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), vec![], &key)?;
/// assert_eq!(package.challenge(), receipt.issued_challenge());
/// let refused = trustzone::make_package("oem", &receipt, &[0x30, 0x03], &[0x04; 33], vec![], &key);
/// assert_eq!(refused.err(), Some(AttestError::AttestedKeyMustBeCompressedP256Sec1));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn make_package(
    oem_id: &str,
    challenge_receipt: &BootChallengeReceipt,
    attested_key_spki_der: &[u8],
    attested_key_compressed_sec1: &[u8],
    cert_chain_der: Vec<Vec<u8>>,
    signing_key: &p256::ecdsa::SigningKey,
) -> Result<ArmTrustZoneEvidencePackage, AttestError> {
    let attested_key = require_compressed_point(attested_key_compressed_sec1)?;
    let quote_der = make_quote(
        oem_id,
        &challenge_receipt.issued_challenge,
        attested_key_spki_der,
        signing_key,
    )?;
    Ok(ArmTrustZoneEvidencePackage {
        quote_der,
        cert_chain_der,
        challenge: challenge_receipt.issued_challenge,
        attested_key_compressed_sec1: attested_key,
        challenge_receipt: challenge_receipt.clone(),
    })
}
