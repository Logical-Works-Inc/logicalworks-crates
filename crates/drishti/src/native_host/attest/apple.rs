//! Apple Secure Enclave package originator: validates the attested key
//! shape, then signs the issued challenge with the device key.

use lgwks_deps::p256;
use p256::ecdsa::signature::Signer as _;

use super::{
    AppleSecureEnclaveEvidencePackage, AttestError, BootChallengeReceipt, require_compressed_point,
};

/// Assemble a package from its parts, signing the issued challenge.
///
/// # Errors
///
/// Returns [`AttestError::AttestedKeyMustBeCompressedP256Sec1`] for a
/// malformed attested key.
/// # Examples
/// ```
/// use drishti::native_host::attest::{AttestError, BootChallengeReceipt, apple};
/// use lgwks_deps::p256;
/// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
/// let point = key.verifying_key().to_sec1_point(true);
/// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
/// let package = apple::make_package(vec![1], vec![], &receipt, point.as_bytes(), &key)?;
/// assert_eq!(package.challenge(), receipt.issued_challenge());
/// // An uncompressed or truncated key is refused before anything is signed.
/// let refused = apple::make_package(vec![1], vec![], &receipt, &[0x04; 33], &key);
/// assert_eq!(refused.err(), Some(AttestError::AttestedKeyMustBeCompressedP256Sec1));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn make_package(
    key_cert_der: Vec<u8>,
    intermediate_certs_der: Vec<Vec<u8>>,
    challenge_receipt: &BootChallengeReceipt,
    attested_key_compressed_sec1: &[u8],
    signing_key: &p256::ecdsa::SigningKey,
) -> Result<AppleSecureEnclaveEvidencePackage, AttestError> {
    let attested = require_compressed_point(attested_key_compressed_sec1)?;
    let signature: p256::ecdsa::Signature = signing_key.sign(&challenge_receipt.issued_challenge);
    Ok(AppleSecureEnclaveEvidencePackage {
        key_cert_der,
        intermediate_certs_der,
        challenge: challenge_receipt.issued_challenge,
        attested_key_compressed_sec1: attested,
        challenge_receipt: challenge_receipt.clone(),
        challenge_pop_signature_der: signature.to_der().as_bytes().to_vec(),
    })
}
