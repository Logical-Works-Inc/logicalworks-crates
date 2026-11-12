//! The AEAD envelope over one sealed record: seal a plaintext under a fresh data
//! key, open it back, and hash the ciphertext with its key identity for the
//! chain.
//!
//! The cipher is ChaCha20-Poly1305 through the `lgwks_deps` storefront
//! (`sealed-aead`), never a direct edge: this module names
//! `lgwks_deps::chacha20poly1305` and nothing else. Every record seals under a
//! data key of its own — minting that key is the [`KeyProvider`]'s
//! job, not this module's — so a nonce reused across records cannot happen: the
//! nonce is drawn fresh per seal from the estate entropy source by the caller,
//! and the key it seals under is never used for another record.
//!
//! [`DataKey`] is move-only and redacted: no `Clone`, no `Copy`, no `Display`,
//! and a `Debug` that names the type and nothing else. Dropping it overwrites
//! the bytes through a compiler fence, because the workspace forbids `unsafe`
//! and a volatile write is unavailable: the overwrite followed by
//! [`std::hint::black_box`] forces the zeroed state to be observable, which is
//! what keeps the store from eliding it.
//!
//! [`KeyProvider`]: crate::sealed::keys::KeyProvider
//! [`DataKey`]: crate::sealed::envelope::DataKey

use lgwks_deps::chacha20poly1305::aead::{Aead, KeyInit};
use lgwks_deps::chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// How many bytes a data key holds: ChaCha20-Poly1305 takes a 256-bit key.
pub(crate) const KEY_BYTES: usize = 32;

/// How many bytes a seal nonce holds: the 96-bit ChaCha20-Poly1305 nonce.
pub(crate) const NONCE_BYTES: usize = 12;

/// How many bytes the authentication tag adds to a ciphertext.
pub(crate) const TAG_BYTES: usize = 16;

/// One record's data key: 32 secret bytes that never reach a log.
///
/// Move-only on purpose. A clone would be a second copy of the key outside the
/// provider's map, and a second copy is a key the provider's destruction would
/// not reach. Callers borrow it for one seal or one open and hand it back by
/// dropping it.
pub struct DataKey {
    /// The secret bytes. Private, borrowed through [`DataKey::bytes`], never
    /// printed, never logged.
    bytes: [u8; KEY_BYTES],
}

impl DataKey {
    /// Hold `bytes` as a data key.
    ///
    /// Crate-internal: keys are minted by the [`KeyProvider`](super::keys::KeyProvider),
    /// never spelled by a caller.
    pub(crate) fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self { bytes }
    }

    /// The secret bytes, borrowed for one seal or one open.
    #[must_use]
    pub(crate) fn bytes(&self) -> &[u8; KEY_BYTES] {
        &self.bytes
    }
}

impl core::fmt::Debug for DataKey {
    /// The type and nothing else: key bytes never reach `Debug` output, logs,
    /// or error chains (CWE-532).
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("DataKey([redacted])")
    }
}

impl Drop for DataKey {
    /// Overwrite the bytes on the way out.
    ///
    /// Best-effort rather than volatile: the workspace forbids `unsafe`, so no
    /// volatile write is available. The overwrite is followed by a
    /// [`std::hint::black_box`] borrow, which forces the zeroed state to be
    /// observable and keeps the store from eliding the write as dead.
    fn drop(&mut self) {
        for slot in self.bytes.iter_mut() {
            *slot = 0;
        }
        std::hint::black_box(&self.bytes);
    }
}

/// Why a seal or an open refused.
///
/// The cipher's own error carries no detail by design (it must not), so these
/// arms name the direction and the store's own length check, and the failing
/// call is logged at the call site before this travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvelopeError {
    /// Sealing the plaintext refused.
    Seal,
    /// Opening the ciphertext refused: the tag did not authenticate, or the
    /// ciphertext is shorter than a tag.
    Open,
}

impl core::fmt::Display for EnvelopeError {
    /// The refusal, naming the direction and never the bytes.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Seal => formatter.write_str("the AEAD refused to seal the record"),
            Self::Open => formatter.write_str(
                "the sealed record did not authenticate under its data key, or is shorter than a tag",
            ),
        }
    }
}

impl std::error::Error for EnvelopeError {}

/// Seal `plaintext` under `key` with `nonce`, returning ciphertext with its
/// tag appended.
///
/// # Errors
///
/// [`EnvelopeError::Seal`] when the cipher refuses. The failure is logged
/// before it travels.
pub(crate) fn seal(
    key: &DataKey,
    nonce: &[u8; NONCE_BYTES],
    plaintext: &[u8],
) -> Result<Vec<u8>, EnvelopeError> {
    let key = Key::from(*key.bytes());
    let cipher = ChaCha20Poly1305::new(&key);
    cipher.encrypt(&Nonce::from(*nonce), plaintext).map_err(|error| {
        lgwks_std::trace::debug!(error = ?error, "envelope-seal: the AEAD refused to seal the record");
        EnvelopeError::Seal
    })
}

/// Open `ciphertext` sealed under `key` with `nonce`.
///
/// # Errors
///
/// [`EnvelopeError::Open`] when the ciphertext is shorter than a tag or the
/// tag does not authenticate. The failure is logged before it travels, without
/// the bytes.
pub(crate) fn open(
    key: &DataKey,
    nonce: &[u8; NONCE_BYTES],
    ciphertext: &[u8],
) -> Result<Vec<u8>, EnvelopeError> {
    if ciphertext.len() < TAG_BYTES {
        lgwks_std::trace::debug!(
            len = ciphertext.len(),
            "envelope-open: the sealed record is shorter than an authentication tag"
        );
        return Err(EnvelopeError::Open);
    }
    let key = Key::from(*key.bytes());
    let cipher = ChaCha20Poly1305::new(&key);
    cipher.decrypt(&Nonce::from(*nonce), ciphertext).map_err(|error| {
        lgwks_std::trace::debug!(error = ?error, "envelope-open: the sealed record did not authenticate");
        EnvelopeError::Open
    })
}

#[cfg(test)]
mod tests {
    //! The envelope's white-box properties: redaction of a key's bytes and the
    //! seal/open round trip. The behaviour over files, restarts and faults is
    //! proved by `tests/it/sealed.rs` and `tests/it/sim_sealed.rs` through the
    //! public interface; these two pin the one thing only white-box access can
    //! name — the exact secret bytes. Keys and nonces come from the entropy
    //! source production provisions from, never a literal: a fixed vector is a
    //! key nobody deploys, and the case a test must hold is the one shipped.

    use super::{DataKey, KEY_BYTES, NONCE_BYTES, TAG_BYTES, open, seal};

    /// What these tests report when a step refuses.
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// `Debug` on a data key names the type and never the bytes (CWE-532):
    /// the exact rendering is pinned, so a key that slips into it fails the
    /// test rather than reaching a log.
    #[test]
    fn debug_names_the_type_and_never_the_key_bytes() -> TestResult {
        let bytes = lgwks_std::random::bytes::<KEY_BYTES>()?;
        let rendered = format!("{:?}", DataKey::from_bytes(bytes));
        assert_eq!(
            rendered, "DataKey([redacted])",
            "a data key renders as its type, never its bytes"
        );
        Ok(())
    }

    /// The RFC 8439 section 2.8.2 key, nonce and plaintext seal to the bytes
    /// `chacha20poly1305` 0.10.1 wrote for them, and those bytes open.
    ///
    /// A sealed record outlives the cipher release that wrote it, so a new
    /// release of the cipher must write and read the same bytes. The first 114
    /// bytes are the RFC's own ciphertext; the last 16 are the tag for an empty
    /// associated data, which the RFC does not print and the 0.10.1 release
    /// produced (2026-11-12). This is the one fixed vector in the module: it is
    /// the published one, a key nobody deploys, and it pins a format rather
    /// than a secret.
    #[test]
    fn a_fixed_vector_seals_to_the_bytes_the_previous_cipher_release_wrote() -> TestResult {
        let key_bytes: [u8; KEY_BYTES] = lgwks_std::hex::decode(
            "808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f",
        )?
        .try_into()
        .map_err(|_refused| "the RFC key is 32 bytes")?;
        let nonce: [u8; NONCE_BYTES] = lgwks_std::hex::decode("070000004041424344454647")?
            .try_into()
            .map_err(|_refused| "the RFC nonce is 12 bytes")?;
        let plaintext: &[u8] = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let key = DataKey::from_bytes(key_bytes);
        let sealed = seal(&key, &nonce, plaintext)?;
        assert_eq!(
            sealed.len(),
            plaintext.len().saturating_add(TAG_BYTES),
            "a seal is its plaintext's length plus one tag"
        );
        assert_eq!(
            lgwks_std::hex::encode(&sealed),
            concat!(
                "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6",
                "3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36",
                "92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc",
                "3ff4def08e4b7a9de576d26586cec64b6116",
                "6a23a4681fd59456aea1d29f82477216",
            ),
            "the seal of the RFC vector is the bytes every earlier release wrote"
        );
        assert_eq!(
            open(&key, &nonce, &sealed)?,
            plaintext,
            "the bytes an earlier release wrote open under this one"
        );
        Ok(())
    }

    /// Seal and open round-trip under a drawn key and nonce, and refuse
    /// another key, another nonce and a truncated ciphertext. "Another" is the
    /// drawn value with one bit flipped, the nearest wrong value there is.
    #[test]
    fn seal_open_round_trip_and_refusals() -> TestResult {
        let key_bytes = lgwks_std::random::bytes::<KEY_BYTES>()?;
        let key = DataKey::from_bytes(key_bytes);
        let nonce = lgwks_std::random::bytes::<NONCE_BYTES>()?;
        let plaintext = lgwks_std::random::bytes::<64>()?;
        let sealed = seal(&key, &nonce, &plaintext)?;
        assert_eq!(
            open(&key, &nonce, &sealed)?,
            plaintext,
            "a seal opens under its own key and nonce"
        );
        let mut other_key = key_bytes;
        other_key[0] ^= 1;
        assert!(
            open(&DataKey::from_bytes(other_key), &nonce, &sealed).is_err(),
            "a seal never opens under another key"
        );
        let mut other_nonce = nonce;
        other_nonce[0] ^= 1;
        assert!(
            open(&key, &other_nonce, &sealed).is_err(),
            "a seal never opens under another nonce"
        );
        assert!(
            open(&key, &nonce, &sealed[..sealed.len().saturating_sub(1)]).is_err(),
            "a truncated seal never opens"
        );
        Ok(())
    }
}
