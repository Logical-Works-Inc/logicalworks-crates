//! Private-CA roots and client identity for `http`, from PEM with no new edge.
//!
//! The default stays what it was: Mozilla's root set over rustls, no client
//! certificate. [`TlsOptions`] replaces that default for callers behind a
//! private CA or behind mutual TLS, and it does so through the `ureq` edge the
//! register already approves (`net.http-client`): ureq's own `tls` module owns
//! PEM-to-DER loading, and `std` still has no PKI loader of its own. The
//! boundary stays inside the approved edge, which is what makes this a seam
//! rather than a new dependency.
//!
//! Loading is strict on purpose. ureq builds its TLS configuration with an
//! `expect` on the client identity, so a key that does not match the
//! certificate would panic inside the dependency instead of failing as a
//! typed error. This module never hands ureq anything it has not already
//! proven: every certificate is walked as DER before it is trusted, every
//! private key is parsed to its public components, and the key is compared
//! against the end-entity certificate field by field. What cannot be proven
//! is refused with [`Error::Failure`] at the TLS stage, never panicked on,
//! and a refusal never echoes key or certificate bytes. Every refusal is
//! announced through `trace` where the refusal is built, so no error leaves
//! this module as a bare value.
//!
//! Admitted material is RSA (up to 8,192 bits) and ECDSA (P-256, P-384,
//! P-521) under the signature schemes the PKI actually negotiates here
//! (SHA-256/384/512 with RSA or ECDSA). Ed25519 identity material is refused
//! with a typed error: proving correspondence needs its curve arithmetic, and
//! the estate's Ed25519 lives in `sign` for detached seals, not in this
//! loader. PEM is the only spelling: a caller holding DER converts it once
//! with `openssl` rather than asking every call site to guess the encoding.
//!
//! [`Error::Failure`]: super::Error::Failure

use super::Error;

mod der;
use der::{CertifiedPublic, KeyPublic, parse_certificate, parse_private_key, validate_certificate};

/// TLS material for one exchange or one [`Client`](super::Client).
///
/// Start from [`TlsOptions::default`] (Mozilla roots, no identity) and add
/// what the deployment needs. Roots replace the default set rather than
/// extending it: a deployment that must reach both a private CA and the
/// public internet keeps the default and terminates the private leg
/// elsewhere. PEM blocks may carry surrounding text and any line endings;
/// certificates are read in order, so the first certificate in the chain PEM
/// is the end-entity the key must match.
///
/// A [`Client`](super::Client) built from options carrying this material
/// uses it for every exchange; the free functions build a one-shot agent
/// from it per call. Either way the material is validated when the agent is
/// built, before any socket opens.
///
/// ```rust
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use lgwks_std::http::{Client, Error, FailureKind, FailureStage, Options, TlsOptions};
/// let options = Options::default()
///     .tls(TlsOptions::default().with_roots_pem(b"not a PEM block".to_vec()));
/// let refused = match Client::new(options) {
///     Ok(_) => return Err("garbage roots must be refused".into()),
///     Err(refused) => refused,
/// };
/// assert!(
///     matches!(
///         refused,
///         Error::Failure { stage: FailureStage::Tls, kind: FailureKind::InvalidRequest, .. }
///     ),
///     "a bad file is a typed TLS refusal, got {refused:?}"
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TlsOptions {
    /// PEM holding the trusted roots, replacing Mozilla's set. `None` keeps
    /// the default.
    roots_pem: Option<Vec<u8>>,
    /// PEM holding the client certificate chain, end-entity first. Always
    /// paired with `client_key_pem`.
    client_chain_pem: Option<Vec<u8>>,
    /// PEM holding exactly one private key for the chain above.
    client_key_pem: Option<Vec<u8>>,
}

impl TlsOptions {
    /// Trust exactly the certificates in `pem`, replacing Mozilla's set.
    ///
    /// Every certificate in the block is structurally validated when the
    /// agent is built; a block with no certificates, or with one that fails
    /// validation, refuses the build with a typed error.
    #[must_use]
    pub fn with_roots_pem(mut self, pem: Vec<u8>) -> Self {
        self.roots_pem = Some(pem);
        self
    }

    /// Present `chain_pem` (end-entity first) as the client certificate.
    ///
    /// Always paired with [`TlsOptions::with_client_key_pem`]: a chain
    /// without a key, or a key without a chain, refuses the build with a
    /// typed error.
    #[must_use]
    pub fn with_client_chain_pem(mut self, chain_pem: Vec<u8>) -> Self {
        self.client_chain_pem = Some(chain_pem);
        self
    }

    /// Present `key_pem` as the private key for the client certificate.
    ///
    /// `key_pem` must hold exactly one private key, and the key must match
    /// the end-entity certificate: RSA moduli and exponents, or curve and
    /// public point, compared field by field. A mismatch is a typed refusal
    /// at build time, not a handshake failure and never a panic.
    #[must_use]
    pub fn with_client_key_pem(mut self, key_pem: Vec<u8>) -> Self {
        self.client_key_pem = Some(key_pem);
        self
    }

    /// Build the ureq TLS configuration this material describes.
    ///
    /// Untouched options return ureq's own default, which is today's behavior
    /// byte for byte: Mozilla roots, no client identity.
    pub(crate) fn tls_config(&self) -> Result<ureq::tls::TlsConfig, Error> {
        if self.roots_pem.is_none()
            && self.client_chain_pem.is_none()
            && self.client_key_pem.is_none()
        {
            return Ok(ureq::tls::TlsConfig::default());
        }
        let mut builder = ureq::tls::TlsConfig::builder();
        if let Some(pem) = self.roots_pem.as_ref() {
            builder = builder.root_certs(roots_from_pem(pem)?);
        }
        builder = builder.client_cert(identity_from_pem(
            self.client_chain_pem.as_ref(),
            self.client_key_pem.as_ref(),
        )?);
        Ok(builder.build())
    }
}

impl std::fmt::Debug for TlsOptions {
    /// Names what is configured without rendering any of it: key and
    /// certificate bytes never reach a log line through this type.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TlsOptions")
            .field("roots", &self.roots_pem.as_ref().map(|pem| pem.len()))
            .field("client_identity", &self.client_chain_pem.is_some())
            .finish()
    }
}

// ── Refusals ────────────────────────────────────────────────────────────────

/// Refuse TLS material with a class-only diagnostic.
///
/// Every detail string is a constant written here: PEM and DER bytes are
/// never echoed into an error, because a key file landing in a log is the
/// failure this whole module exists to prevent. The refusal is announced
/// through `trace` at the same time, so every `return refused(..)` below
/// leaves a signal, not just a value.
fn refused<T>(detail: &'static str) -> Result<T, Error> {
    let error = super::Error::Failure {
        stage: super::FailureStage::Tls,
        kind: super::FailureKind::InvalidRequest,
        cause: Some(super::FailureCause::sanitized(detail.to_owned())),
    };
    #[cfg(feature = "trace")]
    crate::trace::debug!(detail, "tls: refusing material");
    Err(error)
}

// ── Roots and identity ──────────────────────────────────────────────────────

/// Every item of one PEM block, or the block refused as a whole.
///
/// One choke point so every malformed block is announced once, the same way.
fn pem_items(pem: &[u8]) -> Result<Vec<ureq::tls::PemItem<'static>>, ureq::Error> {
    ureq::tls::parse_pem(pem).collect()
}

/// Parse `pem` into exactly the roots it claims, validating each one.
fn roots_from_pem(pem: &[u8]) -> Result<ureq::tls::RootCerts, Error> {
    let items = match pem_items(pem) {
        Ok(items) => items,
        Err(parse_error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%parse_error, "roots PEM is not parseable PEM");
            let _ = parse_error;
            return refused("roots PEM is not parseable PEM");
        }
    };
    let mut roots = Vec::new();
    let mut saw_certificate = false;
    for item in items {
        match item {
            ureq::tls::PemItem::Certificate(certificate) => {
                saw_certificate = true;
                validate_certificate(certificate.der())?;
                roots.push(certificate.to_owned());
            }
            ureq::tls::PemItem::PrivateKey(_) => {
                return refused("roots PEM must hold certificates only");
            }
            _ => return refused("roots PEM holds an unadmitted item"),
        }
    }
    if !saw_certificate {
        return refused("roots PEM holds no certificate");
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&roots))
}

/// Parse the client identity, or `None` when no identity was configured.
///
/// Exactly one of "both halves" and "neither half" is admitted: a chain
/// without a key, or a key without a chain, is a caller bug and refused
/// before ureq ever sees it.
fn identity_from_pem(
    chain_pem: Option<&Vec<u8>>,
    key_pem: Option<&Vec<u8>>,
) -> Result<Option<ureq::tls::ClientCert>, Error> {
    match (chain_pem, key_pem) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            refused("client identity needs both a chain and a key")
        }
        (Some(chain_pem), Some(key_pem)) => {
            let chain_items = match pem_items(chain_pem) {
                Ok(items) => items,
                Err(parse_error) => {
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(%parse_error, "chain PEM is not parseable PEM");
                    let _ = parse_error;
                    return refused("chain PEM is not parseable PEM");
                }
            };
            let mut chain = Vec::new();
            for item in chain_items {
                match item {
                    ureq::tls::PemItem::Certificate(certificate) => {
                        validate_certificate(certificate.der())?;
                        chain.push(certificate.to_owned());
                    }
                    ureq::tls::PemItem::PrivateKey(_) => {
                        return refused("chain PEM must hold certificates only");
                    }
                    _ => return refused("chain PEM holds an unadmitted item"),
                }
            }
            let Some(end_entity) = chain.first() else {
                return refused("chain PEM holds no certificate");
            };
            let key_items = match pem_items(key_pem) {
                Ok(items) => items,
                Err(parse_error) => {
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(%parse_error, "key PEM is not parseable PEM");
                    let _ = parse_error;
                    return refused("key PEM is not parseable PEM");
                }
            };
            let mut keys = Vec::new();
            for item in key_items {
                match item {
                    ureq::tls::PemItem::PrivateKey(key) => keys.push(key.to_owned()),
                    ureq::tls::PemItem::Certificate(_) => {
                        return refused("key PEM must hold a private key only");
                    }
                    _ => return refused("key PEM holds an unadmitted item"),
                }
            }
            if keys.len() != 1 {
                return refused("key PEM must hold exactly one private key");
            }
            let Some(key) = keys.pop() else {
                return refused("key PEM must hold exactly one private key");
            };
            prove_correspondence(end_entity.der(), key.der())?;
            Ok(Some(ureq::tls::ClientCert::new_with_certs(&chain, key)))
        }
    }
}

// ── Correspondence ──────────────────────────────────────────────────────────

/// Prove the private key matches the end-entity certificate.
///
/// RSA compares modulus and exponent against the subject key; EC compares
/// curve and public point. A mismatch is the typed refusal this module
/// exists for: without it, ureq's TLS builder would panic on the same pair.
/// Ed25519 never reaches the comparison: it is refused while parsing the
/// key, with the reason named.
fn prove_correspondence(certificate_der: &[u8], key_der: &[u8]) -> Result<(), Error> {
    let certified = parse_certificate(certificate_der)?;
    let keyed = parse_private_key(key_der)?;
    let matched = match (certified, keyed) {
        (
            CertifiedPublic::Rsa {
                modulus: cert_n,
                exponent: cert_e,
            },
            KeyPublic::Rsa {
                modulus: key_n,
                exponent: key_e,
            },
        ) => cert_n == key_n && cert_e == key_e,
        (
            CertifiedPublic::Ec {
                curve: cert_curve,
                point: cert_point,
            },
            KeyPublic::Ec {
                curve: key_curve,
                point: key_point,
            },
        ) => cert_curve == key_curve && cert_point == key_point,
        _ => false,
    };
    if !matched {
        return refused("private key does not match the certificate");
    }
    Ok(())
}
