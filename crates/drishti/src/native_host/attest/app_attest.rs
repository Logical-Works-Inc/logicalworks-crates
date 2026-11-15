//! App Attest attestation-object parser and the fail-closed production gate.

use lgwks_macros::Error;

use super::super::cbor::{CborValue, decode_attestation};
use super::super::ensure;
use super::{AppleSecureEnclaveEvidencePackage, BootChallengeReceipt};

/// App Attest attestation-object failures.
/// # Examples
/// ```
/// use drishti::native_host::attest::{AppAttestAttestation, AppAttestError};
/// assert_eq!(AppAttestAttestation::parse(&[]).err(), Some(AppAttestError::Truncated));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AppAttestError {
    /// Input ended early (also used for overlong inputs and deep nesting).
    #[error("attestation truncated")]
    Truncated,
    /// CBOR major type outside 0/2/3/4/5.
    #[error("unsupported CBOR major type: {0}")]
    UnsupportedMajorType(u8),
    /// CBOR additional info outside 0-27.
    #[error("unsupported CBOR additional info: {0}")]
    UnsupportedAdditionalInfo(u8),
    /// A length does not fit this platform.
    #[error("CBOR integer overflow")]
    IntegerOverflow(#[from] core::num::TryFromIntError),
    /// A text string is not valid UTF-8.
    #[error("CBOR text is not valid UTF-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    /// The top-level value is not a map.
    #[error("attestation is not a map")]
    NotAMap,
    /// A required key is missing (or present with the wrong shape).
    #[error("attestation missing key: {0}")]
    MissingKey(String),
    /// A value has the wrong shape.
    #[error("attestation unexpected type: {0}")]
    UnexpectedType(String),
    /// The `x5c` chain is empty.
    #[error("attestation has an empty certificate chain")]
    EmptyCertChain,
}

/// Certificate chain plus raw fields from an `apple-appattest` attestation
/// object. `x5c[0]` is the leaf credential certificate; the rest are
/// intermediates toward the pinned root.
///
/// Fields are private: [`AppAttestAttestation::parse`] builds attestations
/// and readers go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::attest::AppAttestAttestation;
/// // {"attStmt": {"x5c": [h'00']}, "authData": h'00'}
/// let object = [
///     0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
///     0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
/// ];
/// let parsed = AppAttestAttestation::parse(&object)?;
/// assert_eq!(parsed.chain().len(), 1);
/// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppAttestAttestation {
    /// Certificate chain DERs, leaf first.
    x5c: Vec<Vec<u8>>,
    /// Authenticator data bytes.
    auth_data: Vec<u8>,
    /// Optional receipt bytes.
    receipt: Option<Vec<u8>>,
}

impl AppAttestAttestation {
    /// Parse an attestation object, pulling out the `x5c` chain, `authData`,
    /// and optional `receipt`. Fails closed on malformed CBOR, a missing or
    /// empty `x5c`, or an unexpected shape — never a partially-trusted
    /// result.
    ///
    /// # Errors
    ///
    /// Returns the [`AppAttestError`] case matching the first defect found.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::{AppAttestAttestation, AppAttestError};
    /// // A bare byte string is valid CBOR but not an attestation map.
    /// let not_a_map = AppAttestAttestation::parse(&[0x41, 0x00]);
    /// assert_eq!(not_a_map.err(), Some(AppAttestError::NotAMap));
    /// // An empty map lacks `attStmt`.
    /// let empty = AppAttestAttestation::parse(&[0xA0]);
    /// assert_eq!(empty.err(), Some(AppAttestError::MissingKey("attStmt".into())));
    /// ```
    pub fn parse(cbor: &[u8]) -> Result<Self, AppAttestError> {
        let root = decode_attestation(cbor)?;
        let missing = |key: &str| AppAttestError::MissingKey(String::from(key));
        let top = root.map_value().ok_or(AppAttestError::NotAMap)?;
        let att_stmt = top
            .get("attStmt")
            .and_then(CborValue::map_value)
            .ok_or_else(|| missing("attStmt"))?;
        let chain_items = att_stmt
            .get("x5c")
            .and_then(CborValue::array_value)
            .ok_or_else(|| missing("x5c"))?;
        let chain = chain_items
            .iter()
            .map(|item| {
                item.bytes_value()
                    .cloned()
                    .ok_or_else(|| AppAttestError::UnexpectedType(String::from("x5c entry")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        ensure(!chain.is_empty(), || AppAttestError::EmptyCertChain)?;
        let auth_data = top
            .get("authData")
            .and_then(CborValue::bytes_value)
            .ok_or_else(|| missing("authData"))?;
        // Absent is fine; present with any type but a byte string is not.
        let receipt = att_stmt
            .get("receipt")
            .map(|value| {
                value
                    .bytes_value()
                    .cloned()
                    .ok_or_else(|| AppAttestError::UnexpectedType(String::from("receipt")))
            })
            .transpose()?;
        Ok(Self {
            x5c: chain,
            auth_data: auth_data.clone(),
            receipt,
        })
    }

    /// Leaf credential certificate DER (its SPKI carries the attested key).
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::AppAttestAttestation;
    /// let object = [
    ///     0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
    ///     0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
    /// ];
    /// assert_eq!(AppAttestAttestation::parse(&object)?.leaf_cert_der(), [0x00]);
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn leaf_cert_der(&self) -> &[u8] {
        match self.x5c.first() {
            Some(leaf) => leaf,
            None => &[],
        }
    }

    /// Intermediate certificate DERs, in chain order toward the root.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::AppAttestAttestation;
    /// let object = [
    ///     0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
    ///     0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
    /// ];
    /// // The one-certificate chain has a leaf and no intermediates.
    /// assert!(AppAttestAttestation::parse(&object)?.intermediate_certs_der().is_empty());
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn intermediate_certs_der(&self) -> &[Vec<u8>] {
        match self.x5c.get(1..) {
            Some(rest) => rest,
            None => &[],
        }
    }

    /// Certificate chain DERs, leaf first.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::AppAttestAttestation;
    /// let object = [
    ///     0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
    ///     0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
    /// ];
    /// assert_eq!(AppAttestAttestation::parse(&object)?.chain(), [vec![0x00]]);
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn chain(&self) -> &[Vec<u8>] {
        &self.x5c
    }

    /// The WebAuthn-style authenticator blob (RP id hash, flags, counter) that
    /// the attestation signs, exactly as the device sent it.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::AppAttestAttestation;
    /// let object = [
    ///     0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
    ///     0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
    /// ];
    /// assert_eq!(AppAttestAttestation::parse(&object)?.auth_data(), [0x00]);
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub fn auth_data(&self) -> &[u8] {
        &self.auth_data
    }

    /// Optional receipt bytes.
    /// # Examples
    /// ```
    /// use drishti::native_host::attest::AppAttestAttestation;
    /// let object = [
    ///     0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
    ///     0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
    /// ];
    /// // The object carries no `receipt` key.
    /// assert!(AppAttestAttestation::parse(&object)?.receipt().is_none());
    /// # Ok::<(), drishti::native_host::attest::AppAttestError>(())
    /// ```
    #[must_use]
    pub const fn receipt(&self) -> Option<&Vec<u8>> {
        self.receipt.as_ref()
    }
}

/// App Attest production failures.
/// # Examples
/// ```
/// use drishti::native_host::attest::AppAttestProductionError;
/// assert_eq!(AppAttestProductionError::AppAttestUnavailable.to_string(), "App Attest unavailable");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AppAttestProductionError {
    /// App Attest is unavailable in this process: fail closed, never a soft
    /// fallback to a lower hardware tier.
    #[error("App Attest unavailable")]
    AppAttestUnavailable,
    /// The entitlement-gated production path is deferred to kernel #569.
    #[error("App Attest production path deferred to kernel #569")]
    ProductionPathNotYetImplemented,
}

/// True only when this process can actually perform App Attest. Always false
/// off-device: no entitlement or provisioning exists here.
/// # Examples
/// ```
/// use drishti::native_host::attest::app_attest_is_supported;
/// assert!(!app_attest_is_supported());
/// ```
#[must_use]
pub const fn app_attest_is_supported() -> bool {
    false
}

/// Produce App Attest evidence for a challenge receipt. Refuses while the
/// on-device path is deferred.
///
/// # Errors
///
/// Returns [`AppAttestProductionError::AppAttestUnavailable`] off-device.
/// # Examples
/// ```
/// use drishti::native_host::attest::{AppAttestProductionError, BootChallengeReceipt, produce_app_attest_evidence};
/// let receipt = BootChallengeReceipt::new(&[1; 32], 1, 2, &[2; 32], 1, &[3; 32])?;
/// let refused = produce_app_attest_evidence(&receipt);
/// assert_eq!(refused.err(), Some(AppAttestProductionError::AppAttestUnavailable));
/// # Ok::<(), drishti::native_host::attest::AttestError>(())
/// ```
pub fn produce_app_attest_evidence(
    _receipt: &BootChallengeReceipt,
) -> Result<AppleSecureEnclaveEvidencePackage, AppAttestProductionError> {
    ensure(app_attest_is_supported(), || {
        AppAttestProductionError::AppAttestUnavailable
    })?;
    Err(AppAttestProductionError::ProductionPathNotYetImplemented)
}
