//! The AEAD envelope over one sealed record: seal a plaintext under a fresh data
//! key, open it back, and hash the ciphertext with its key identity for the
//! chain.
//!
//! The cipher is ChaCha20-Poly1305 through the `lgwks_deps` storefront
//! (`sealed-aead`), never a direct edge: this module names
//! `lgwks_deps::chacha20poly1305` and nothing else. Every record seals under a
//! data key of its own — minting that key is the [`KeyProvider`](super::keys::KeyProvider)'s
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
    //! The envelope's white-box properties: redaction with known bytes and a
    //! deterministic seal/open round trip. The behaviour over files, restarts
    //! and faults is proved by `tests/it/sealed.rs` and
    //! `tests/it/sim_sealed.rs` through the public interface; these two pin
    //! the one thing only white-box access can name — the exact secret bytes.

    use super::{DataKey, EnvelopeError, KEY_BYTES, NONCE_BYTES, open, seal};

    /// `Debug` on a data key names the type and never the bytes (CWE-532):
    /// the exact rendering is pinned, so a key that slips into it fails the
    /// test rather than reaching a log.
    #[test]
    fn debug_names_the_type_and_never_the_key_bytes() {
        let rendered = format!("{:?}", DataKey::from_bytes([0xAB_u8; KEY_BYTES]));
        assert_eq!(
            rendered, "DataKey([redacted])",
            "a data key renders as its type, never its bytes"
        );
    }

    /// Seal and open round-trip under fixed vectors, and refuse a wrong key,
    /// a wrong nonce and a truncated ciphertext — deterministically, with no
    /// entropy and no clock.
    #[test]
    fn seal_open_round_trip_and_refusals() -> Result<(), EnvelopeError> {
        let key = DataKey::from_bytes([0x11_u8; KEY_BYTES]);
        let nonce = [0x22_u8; NONCE_BYTES];
        let plaintext = b"a fixed test vector";
        let sealed = seal(&key, &nonce, plaintext)?;
        assert_eq!(
            open(&key, &nonce, &sealed)?,
            plaintext,
            "a seal opens under its own key and nonce"
        );
        assert!(
            open(&DataKey::from_bytes([0x33_u8; KEY_BYTES]), &nonce, &sealed).is_err(),
            "a seal never opens under another key"
        );
        assert!(
            open(&key, &[0x44_u8; NONCE_BYTES], &sealed).is_err(),
            "a seal never opens under another nonce"
        );
        assert!(
            open(&key, &nonce, &sealed[..sealed.len().saturating_sub(1)]).is_err(),
            "a truncated seal never opens"
        );
        Ok(())
    }
}
