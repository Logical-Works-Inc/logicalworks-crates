//! Attestation evidence: secure-enclave and TrustZone packages, App Attest parsing.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasBroker/
//! AttestationEvidence.swift`, `AppAttestAttestation.swift`, and
//! `AppleAppAttestEvidenceProducer.swift` (kernel#850 row 15). P-256 comes
//! from the `p256` crate (RustCrypto), whose `sign`/`verify` prehash with
//! SHA-256 exactly like the platform signing the kernel used, so proof-of-
//! possession semantics carry over. The DER writer and the CBOR subset reader
//! ([`crate::native_host::cbor`]) are hand-rolled like the kernel's.
//!
//! The entitlement-gated on-device production path stays fail-closed:
//! [`app_attest_is_supported`] is false off-device and
//! [`produce_app_attest_evidence`] refuses, mirroring the kernel's deferred
//! `#569` path. Verification against the kernel's `canvas-attest-verify`
//! belongs to the secure-authority absorption row, not this one.
//!
//! Split for the size law: the App Attest object parser and the production
//! gate live in `app_attest`, the originators in `apple` and `trustzone`, the
//! DER writer in `der`; this root owns the error, the challenge receipt, and
//! the two evidence package types.

use lgwks_deps::p256;
use lgwks_macros::Error;

use super::ensure;

/// App Attest object parser and production gate (split for the size law).
mod app_attest;
/// Apple Secure Enclave package originator (split for the size law).
pub mod apple;
/// Minimal DER writer (split for the size law).
pub mod der;
/// ARM TrustZone quote and package originator (split for the size law).
pub mod trustzone;

pub use app_attest::{
    AppAttestAttestation, AppAttestError, AppAttestProductionError, app_attest_is_supported,
    produce_app_attest_evidence,
};

/// Attestation originator failures.
/// # Examples
/// ```
/// use drishti::native_host::attest::{AttestError, BootChallengeReceipt};
/// let short = BootChallengeReceipt::new(&[0; 31], 0, 0, &[0; 32], 0, &[0; 32]);
/// assert_eq!(short.err(), Some(AttestError::ChallengeMustBe32Bytes));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AttestError {
    /// A challenge, receipt, or MAC is not 32 bytes.
    #[error("challenge must be 32 bytes")]
    ChallengeMustBe32Bytes,
    /// The attested key is not a 33-byte compressed P-256 SEC1 point.
    #[error("attested key must be compressed P-256 SEC1")]
    AttestedKeyMustBeCompressedP256Sec1,
}

/// Boot challenge receipt binding issuance to consumption.
///
/// Fields are private: the validating [`BootChallengeReceipt::new`] builds
/// receipts and readers go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::attest::BootChallengeReceipt;
/// let receipt = BootChallengeReceipt::new(&[1; 32], 1_000, 2_000, &[2; 32], 1_500, &[3; 32])?;
/// assert_eq!((receipt.issued_at_epoch(), receipt.expires_at_epoch()), (1_000, 2_000));
/// # Ok::<(), drishti::native_host::attest::AttestError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BootChallengeReceipt {
    /// Issued 32-byte challenge.
    issued_challenge: [u8; 32],
    /// Issue time, epoch seconds.
    issued_at_epoch: u64,
    /// Expiry time, epoch seconds.
    expires_at_epoch: u64,
    /// Consumed 32-byte challenge.
    consumed_challenge: [u8; 32],
    /// Consumption time, epoch seconds.
    consumed_at_epoch: u64,
    /// 32-byte issue MAC.
    issue_mac: [u8; 32],
}

impl BootChallengeReceipt {
    /// Assemble a receipt, refusing non-32-byte challenges and MACs.
    ///
    /// # Errors
    ///
    /// Returns [`AttestError::ChallengeMustBe32Bytes`] for any wrongly sized
    /// input.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{AttestError, BootChallengeReceipt};
    /// assert!(BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32]).is_ok());
    /// // Each of the three byte strings is checked on its own.
    /// let bad_mac = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 33]);
    /// assert_eq!(bad_mac.err(), Some(AttestError::ChallengeMustBe32Bytes));
    /// ```
    pub fn new(
        issued_challenge: &[u8],
        issued_at_epoch: u64,
        expires_at_epoch: u64,
        consumed_challenge: &[u8],
        consumed_at_epoch: u64,
        issue_mac: &[u8],
    ) -> Result<Self, AttestError> {
        Ok(Self {
            issued_challenge: require_32(issued_challenge)?,
            issued_at_epoch,
            expires_at_epoch,
            consumed_challenge: require_32(consumed_challenge)?,
            consumed_at_epoch,
            issue_mac: require_32(issue_mac)?,
        })
    }

    /// Challenge the device was asked to sign.
    /// # Examples
    /// ```
    /// # use drishti::native_host::attest::BootChallengeReceipt;
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// assert_eq!(receipt.issued_challenge(), [1; 32]);
    /// # Ok::<(), drishti::native_host::attest::AttestError>(())
    /// ```
    #[must_use]
    pub const fn issued_challenge(&self) -> [u8; 32] {
        self.issued_challenge
    }

    /// Seconds since the Unix epoch when the issuer minted the challenge; the
    /// receipt is only fresh between this and `expires_at_epoch`.
    /// # Examples
    /// ```
    /// # use drishti::native_host::attest::BootChallengeReceipt;
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 10, 20, &[2; 32], 15, &[3; 32])?;
    /// assert_eq!(receipt.issued_at_epoch(), 10);
    /// # Ok::<(), drishti::native_host::attest::AttestError>(())
    /// ```
    #[must_use]
    pub const fn issued_at_epoch(&self) -> u64 {
        self.issued_at_epoch
    }

    /// Expiry time, epoch seconds.
    /// # Examples
    /// ```
    /// # use drishti::native_host::attest::BootChallengeReceipt;
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 10, 20, &[2; 32], 15, &[3; 32])?;
    /// assert_eq!(receipt.expires_at_epoch(), 20);
    /// # Ok::<(), drishti::native_host::attest::AttestError>(())
    /// ```
    #[must_use]
    pub const fn expires_at_epoch(&self) -> u64 {
        self.expires_at_epoch
    }

    /// Challenge the device presented back when the receipt was consumed.
    /// # Examples
    /// ```
    /// # use drishti::native_host::attest::BootChallengeReceipt;
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// assert_eq!(receipt.consumed_challenge(), [2; 32]);
    /// # Ok::<(), drishti::native_host::attest::AttestError>(())
    /// ```
    #[must_use]
    pub const fn consumed_challenge(&self) -> [u8; 32] {
        self.consumed_challenge
    }

    /// Consumption time, epoch seconds.
    /// # Examples
    /// ```
    /// # use drishti::native_host::attest::BootChallengeReceipt;
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 10, 20, &[2; 32], 15, &[3; 32])?;
    /// assert_eq!(receipt.consumed_at_epoch(), 15);
    /// # Ok::<(), drishti::native_host::attest::AttestError>(())
    /// ```
    #[must_use]
    pub const fn consumed_at_epoch(&self) -> u64 {
        self.consumed_at_epoch
    }

    /// MAC the issuer attached when it minted the challenge.
    /// # Examples
    /// ```
    /// # use drishti::native_host::attest::BootChallengeReceipt;
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// assert_eq!(receipt.issue_mac(), [3; 32]);
    /// # Ok::<(), drishti::native_host::attest::AttestError>(())
    /// ```
    #[must_use]
    pub const fn issue_mac(&self) -> [u8; 32] {
        self.issue_mac
    }
}

/// Copy `bytes` into an array of exactly `N` bytes; any other length is none.
fn exact<const N: usize>(bytes: &[u8]) -> Option<[u8; N]> {
    bytes
        .first_chunk::<N>()
        .copied()
        .filter(|_| bytes.len() == N)
}

/// Copy a 32-byte slice into an array, refusing any other length.
fn require_32(bytes: &[u8]) -> Result<[u8; 32], AttestError> {
    exact(bytes).ok_or(AttestError::ChallengeMustBe32Bytes)
}

/// Validate a compressed SEC1 P-256 point: 33 bytes, tag `02` or `03`, and an
/// x coordinate that lies on the curve. A key that only looks the part would
/// let an attestation name a key no device can hold.
fn require_compressed_point(key: &[u8]) -> Result<[u8; 33], AttestError> {
    let refusal = || AttestError::AttestedKeyMustBeCompressedP256Sec1;
    let point = exact::<33>(key).ok_or_else(refusal)?;
    ensure(matches!(point.first(), Some(0x02 | 0x03)), refusal)?;
    ensure(
        p256::ecdsa::VerifyingKey::from_sec1_bytes(&point).is_ok(),
        refusal,
    )?;
    Ok(point)
}

/// Apple Secure Enclave evidence package (frozen wire form).
///
/// Fields are private: the [`apple`] originator builds packages and readers
/// go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::attest::{BootChallengeReceipt, apple};
/// use lgwks_deps::p256;
/// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
/// let point = key.verifying_key().to_sec1_point(true);
/// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
/// let package = apple::make_package(vec![0x30], vec![], &receipt, point.as_bytes(), &key)?;
/// assert_eq!(package.challenge(), [1; 32]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppleSecureEnclaveEvidencePackage {
    /// Leaf credential certificate DER.
    key_cert_der: Vec<u8>,
    /// Intermediate CA DERs toward the pinned root.
    intermediate_certs_der: Vec<Vec<u8>>,
    /// Boot challenge bound by proof-of-possession.
    challenge: [u8; 32],
    /// 33-byte compressed SEC1 attested key.
    attested_key_compressed_sec1: [u8; 33],
    /// Challenge receipt the signature covers.
    challenge_receipt: BootChallengeReceipt,
    /// DER-encoded P-256 proof-of-possession signature.
    challenge_pop_signature_der: Vec<u8>,
}

impl AppleSecureEnclaveEvidencePackage {
    /// The device's own certificate, whose public key the verifier checks the
    /// proof-of-possession signature against.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, apple};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = apple::make_package(vec![0x30, 0x01], vec![], &receipt, point.as_bytes(), &key)?;
    /// assert_eq!(package.key_cert_der(), [0x30, 0x01]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn key_cert_der(&self) -> &[u8] {
        &self.key_cert_der
    }

    /// Intermediate CA DERs toward the pinned root.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, apple};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = apple::make_package(vec![1], vec![vec![2], vec![3]], &receipt, point.as_bytes(), &key)?;
    /// assert_eq!(package.intermediate_certs_der().len(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn intermediate_certs_der(&self) -> &[Vec<u8>] {
        &self.intermediate_certs_der
    }

    /// Boot challenge bound by proof-of-possession.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, apple};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[7; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = apple::make_package(vec![1], vec![], &receipt, point.as_bytes(), &key)?;
    /// // The package binds the issued challenge, not the consumed one.
    /// assert_eq!(package.challenge(), [7; 32]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn challenge(&self) -> [u8; 32] {
        self.challenge
    }

    /// The key under attestation as a 33-byte compressed SEC1 point, already
    /// checked to lie on the curve when the package was built.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, apple};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = apple::make_package(vec![1], vec![], &receipt, point.as_bytes(), &key)?;
    /// assert_eq!(package.attested_key_compressed_sec1().as_slice(), point.as_bytes());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn attested_key_compressed_sec1(&self) -> [u8; 33] {
        self.attested_key_compressed_sec1
    }

    /// The receipt whose issued challenge the proof-of-possession signature
    /// signs; a verifier replays its freshness window from this.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, apple};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = apple::make_package(vec![1], vec![], &receipt, point.as_bytes(), &key)?;
    /// assert_eq!(package.challenge_receipt(), &receipt);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn challenge_receipt(&self) -> &BootChallengeReceipt {
        &self.challenge_receipt
    }

    /// DER-encoded P-256 proof-of-possession signature.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, apple};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = apple::make_package(vec![1], vec![], &receipt, point.as_bytes(), &key)?;
    /// // A DER ECDSA signature is a SEQUENCE (tag 0x30).
    /// assert_eq!(package.challenge_pop_signature_der().first(), Some(&0x30));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn challenge_pop_signature_der(&self) -> &[u8] {
        &self.challenge_pop_signature_der
    }
}

/// ARM TrustZone evidence package.
///
/// Fields are private: the [`trustzone`] producer builds packages and readers
/// go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::attest::{BootChallengeReceipt, trustzone};
/// use lgwks_deps::p256;
/// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
/// let point = key.verifying_key().to_sec1_point(true);
/// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
/// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), vec![], &key)?;
/// assert_eq!(package.challenge(), [1; 32]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArmTrustZoneEvidencePackage {
    /// DER quote: outer `SEQ { OCTET body, OCTET signature }`.
    quote_der: Vec<u8>,
    /// Certificate chain DERs.
    cert_chain_der: Vec<Vec<u8>>,
    /// Boot challenge quoted.
    challenge: [u8; 32],
    /// 33-byte compressed SEC1 attested key.
    attested_key_compressed_sec1: [u8; 33],
    /// Challenge receipt the quote covers.
    challenge_receipt: BootChallengeReceipt,
}

impl ArmTrustZoneEvidencePackage {
    /// The signed quote: an outer sequence of the quoted body and the
    /// device's ECDSA signature over it.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, trustzone};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), vec![], &key)?;
    /// // The quote is the outer SEQUENCE the producer wrapped.
    /// assert_eq!(package.quote_der().first(), Some(&0x30));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn quote_der(&self) -> &[u8] {
        &self.quote_der
    }

    /// The receipt whose issued challenge the quote embeds; a verifier replays
    /// its freshness window from this.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, trustzone};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), vec![], &key)?;
    /// assert_eq!(package.challenge_receipt(), &receipt);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn challenge_receipt(&self) -> &BootChallengeReceipt {
        &self.challenge_receipt
    }

    /// Boot challenge quoted.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, trustzone};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[9; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), vec![], &key)?;
    /// assert_eq!(package.challenge(), [9; 32]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn challenge(&self) -> [u8; 32] {
        self.challenge
    }

    /// Certificates, in the order the producer supplied them, that chain the
    /// quoting key toward the OEM root.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, trustzone};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let chain = vec![vec![1], vec![2]];
    /// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), chain.clone(), &key)?;
    /// assert_eq!(package.cert_chain_der(), chain.as_slice());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn cert_chain_der(&self) -> &[Vec<u8>] {
        &self.cert_chain_der
    }

    /// 33-byte compressed SEC1 attested key covered by the quote.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{BootChallengeReceipt, trustzone};
    /// use lgwks_deps::p256;
    /// let key = p256::ecdsa::SigningKey::from_slice(&[0x42; 32])?;
    /// let point = key.verifying_key().to_sec1_point(true);
    /// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
    /// let package = trustzone::make_package("oem", &receipt, &[0x30, 0x03], point.as_bytes(), vec![], &key)?;
    /// assert_eq!(package.attested_key_compressed_sec1().as_slice(), point.as_bytes());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn attested_key_compressed_sec1(&self) -> [u8; 33] {
        self.attested_key_compressed_sec1
    }
}
