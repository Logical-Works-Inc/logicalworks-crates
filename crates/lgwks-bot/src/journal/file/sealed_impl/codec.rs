//! The read side of the sealed frame codec: one committed sealed payload
//! classified at open as a live, erased or unreadable record, a tombstone, or
//! the damage that stops it becoming a slot.

use std::sync::Arc;
use std::time::Duration;

use crate::journal::file::{EffectEvent, WireError, from_bytes};
use crate::sealed::envelope::{NONCE_BYTES, open as envelope_open};
use crate::sealed::fold::{Unwrapped, fold_unwrap};
use crate::sealed::keys::{KeyId, KeyProvider};
use crate::sealed::record::{DecodedFrame, decode_frame};

/// One sealed frame the scan classified: a record with its opened state, or
/// a tombstone naming a record frame.
pub(in crate::journal::file) enum SealedClassified {
    /// A sealed record: the slot the scan folds, and the live event when the
    /// key opened.
    Record {
        /// The data key's identity.
        key: KeyId,
        /// The wrapped data key, as the frame stored it.
        wrapped: Vec<u8>,
        /// The seal nonce, as the frame stored it.
        nonce: [u8; NONCE_BYTES],
        /// The ciphertext, as the frame stored it.
        ciphertext: Vec<u8>,
        /// The plaintext routing fingerprint, as the frame stored it.
        fingerprint: [u8; 32],
        /// When the record expires, as the frame stored it.
        expires_at: Option<Duration>,
        /// The decrypted event, or `None` when the key is destroyed or the
        /// provider would not answer. Reads retry the unwrap, so a healed
        /// provider reads live again.
        event: Option<Box<EffectEvent>>,
        /// Whether the key is destroyed: reads as erased.
        erased: bool,
    },
    /// A tombstone: the shredded record's frame ordinal and key identity.
    Tombstone {
        /// The shredded record's frame ordinal.
        index: u64,
        /// The shredded record's key identity.
        key_id: [u8; 16],
    },
}

/// What opening one sealed frame found: not a sealed frame, or the reason it
/// cannot become a slot.
pub(in crate::journal::file) enum SealedDamage {
    /// The payload is not a sealed frame: the caller decides whether that is
    /// mode mixing or rot.
    NotSealed,
    /// The frame names another tenant's namespace, carries no routing
    /// fingerprint, or was written by no provider of this file.
    Foreign,
    /// The envelope does not open under its live key, or the plaintext is
    /// not an event: committed bytes that no longer mean what the chain says.
    Misshapen,
}

/// Classify one sealed frame payload at open: decode the envelope, restore
/// the live key, and decrypt — or mark the slot erased or unreadable.
///
/// The scan maps every refusal onto the frame's own index: a frame the
/// provider cannot answer is unreadable rather than absent, and a frame no
/// provider of this file wrote is refused rather than trimmed.
pub(in crate::journal::file) fn classify_sealed_frame(
    payload: &[u8],
    tenant: &str,
    provider: &Arc<dyn KeyProvider>,
    at: u64,
) -> Result<SealedClassified, SealedDamage> {
    let Some(decoded) = decode_frame(payload) else {
        lgwks_std::trace::debug!(
            at = at,
            "classify_sealed_frame: the payload is no sealed frame"
        );
        return Err(SealedDamage::NotSealed);
    };
    match decoded {
        DecodedFrame::Record {
            tenant: frame_tenant,
            key_id,
            wrapped,
            nonce,
            ciphertext,
            expires_at,
            bind,
        } => {
            if frame_tenant != tenant {
                lgwks_std::trace::debug!(
                    at,
                    "sealed-scan: a frame names another tenant's namespace"
                );
                return Err(SealedDamage::Foreign);
            }
            let Ok(fingerprint) = <[u8; 32]>::try_from(bind) else {
                lgwks_std::trace::debug!(at, "sealed-scan: a frame carries no routing fingerprint");
                return Err(SealedDamage::Foreign);
            };
            let key = KeyId::new(tenant.to_owned(), key_id);
            if let Err(error) = provider.note_wrapped(&key, &wrapped) {
                lgwks_std::trace::debug!(at, error = ?error, "sealed-scan: a live record's key does not open");
                return Err(SealedDamage::Foreign);
            }
            match fold_unwrap(provider.unwrap_key(&key, &wrapped)) {
                Ok(Unwrapped::Live(data_key)) => {
                    match envelope_open(&data_key, &nonce, &ciphertext) {
                        Ok(plaintext) => match from_bytes::<EffectEvent, WireError>(&plaintext) {
                            Ok(event) => Ok(SealedClassified::Record {
                                key,
                                wrapped,
                                nonce,
                                ciphertext,
                                expires_at,
                                fingerprint,
                                event: Some(Box::new(event)),
                                erased: false,
                            }),
                            Err(error) => {
                                lgwks_std::trace::debug!(
                                    at,
                                    ?error,
                                    "sealed-scan: a committed frame does not decode"
                                );
                                Err(SealedDamage::Misshapen)
                            }
                        },
                        Err(error) => {
                            lgwks_std::trace::debug!(at, error = ?error, "sealed-scan: a live key with an unopenable envelope");
                            Err(SealedDamage::Misshapen)
                        }
                    }
                }
                Ok(Unwrapped::Erased) => Ok(SealedClassified::Record {
                    key,
                    wrapped,
                    nonce,
                    ciphertext,
                    expires_at,
                    fingerprint,
                    event: None,
                    erased: true,
                }),
                Err(error) => {
                    lgwks_std::trace::debug!(at, error = ?error, "sealed-scan: a record the provider cannot answer yet");
                    Ok(SealedClassified::Record {
                        key,
                        wrapped,
                        nonce,
                        ciphertext,
                        expires_at,
                        fingerprint,
                        event: None,
                        erased: false,
                    })
                }
            }
        }
        DecodedFrame::Tombstone { index, key_id } => {
            Ok(SealedClassified::Tombstone { index, key_id })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{SealedClassified, SealedDamage, classify_sealed_frame};
    use crate::effect::{
        ActionDigest, ActionId, AttemptId, EffectIdentity, EnvironmentEpoch, EnvironmentId,
        FlowRevision, RunId,
    };
    use crate::journal::file::EffectEvent;
    use crate::sealed::envelope::{DataKey, KeyMint, seal};
    use crate::sealed::keys::{KeyError, KeyId, KeyProvider, SoftwareProvider};
    use crate::sealed::record::{encode_record, encode_tombstone};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Removes the provider directory however the test ends.
    struct DirGuard(std::path::PathBuf);

    impl Drop for DirGuard {
        fn drop(&mut self) {
            match std::fs::remove_dir_all(&self.0) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => lgwks_std::trace::debug!(%error, "codec test: scratch dir kept"),
            }
        }
    }

    /// A provider that holds every key but cannot answer an unwrap: the
    /// transient refusal a remote key service gives while it is down.
    struct Silent(Arc<dyn KeyProvider>);

    impl KeyProvider for Silent {
        fn provision_with(
            &self,
            tenant: &str,
            mint: KeyMint,
        ) -> Result<(KeyId, DataKey), KeyError> {
            self.0.provision_with(tenant, mint)
        }

        fn wrap_key(&self, id: &KeyId) -> Result<Vec<u8>, KeyError> {
            self.0.wrap_key(id)
        }

        fn unwrap_with(
            &self,
            _id: &KeyId,
            _wrapped: &[u8],
            _mint: KeyMint,
        ) -> Result<DataKey, KeyError> {
            Err(KeyError::Transient {
                reason: "the key service is not answering",
            })
        }

        fn destroy(&self, id: &KeyId) -> Result<(), KeyError> {
            self.0.destroy(id)
        }

        fn note_wrapped(&self, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError> {
            self.0.note_wrapped(id, wrapped)
        }
    }

    /// One admitted intent, the event every frame below seals.
    fn event() -> Result<EffectEvent, Box<dyn std::error::Error>> {
        let identity = EffectIdentity::new(
            RunId::from_hex("0102030405060708090a0b0c0d0e0f10")?,
            EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?,
            FlowRevision::from_tagged("blake3_256", &"00".repeat(32))?,
        );
        let key = identity.key(
            ActionId::from_hex("1112131415161718191a1b1c1d1e1f20")?,
            AttemptId::from_decimal("1")?,
            ActionDigest::from_tagged("blake3_256", &"f0".repeat(32))?,
            EnvironmentEpoch::from_decimal("1")?,
        );
        Ok(EffectEvent::IntentAdmitted { key })
    }

    /// A live sealed record classifies with its event, a neighbour tenant is
    /// refused as foreign, a destroyed key reads as erased with no event, a
    /// tombstone keeps its ordinal, and a plain payload is no sealed frame.
    #[test]
    fn codec_classifies_live_foreign_erased_tombstone_and_plain() -> TestResult {
        let nonce_hex: [u8; 8] = lgwks_std::random::bytes::<8>()?;
        let dir = std::env::temp_dir().join(format!(
            "lgwks-codec-{}",
            nonce_hex
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
        let _guard = DirGuard(dir.clone());
        let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
        let (id, data_key) = provider.provision("acme")?;
        let nonce: [u8; 12] = lgwks_std::random::bytes::<12>()?;
        let plaintext = event()?.to_bytes()?.as_ref().to_vec();
        let ciphertext = seal(&data_key, &nonce, &plaintext)?;
        let wrapped = provider.wrap_key(&id)?;
        let payload = encode_record(
            "acme",
            &id.id(),
            &wrapped,
            &nonce,
            &ciphertext,
            None,
            &[9; 32],
        );

        let silent: Arc<dyn KeyProvider> = Arc::new(Silent(Arc::clone(&provider)));
        let unanswered = classify_sealed_frame(&payload, "acme", &silent, 0);
        assert!(
            matches!(
                unanswered,
                Ok(SealedClassified::Record {
                    erased: false,
                    event: None,
                    ..
                })
            ),
            "a provider that cannot answer leaves the record unreadable, never erased"
        );
        let live = classify_sealed_frame(&payload, "acme", &provider, 0);
        assert!(
            matches!(
                live,
                Ok(SealedClassified::Record {
                    erased: false,
                    event: Some(_),
                    ..
                })
            ),
            "a live key opens its record"
        );
        assert!(
            matches!(
                classify_sealed_frame(&payload, "globex", &provider, 0),
                Err(SealedDamage::Foreign)
            ),
            "a neighbour tenant's frame is foreign"
        );
        provider.destroy(&id)?;
        let erased = classify_sealed_frame(&payload, "acme", &provider, 0);
        assert!(
            matches!(
                erased,
                Ok(SealedClassified::Record {
                    erased: true,
                    event: None,
                    ..
                })
            ),
            "a destroyed key reads as erased, never as bytes"
        );
        let tombstone = classify_sealed_frame(&encode_tombstone(3, &id.id()), "acme", &provider, 1);
        assert!(
            matches!(tombstone, Ok(SealedClassified::Tombstone { index: 3, .. })),
            "a tombstone keeps the ordinal it names"
        );
        assert!(
            matches!(
                classify_sealed_frame(b"plain", "acme", &provider, 2),
                Err(SealedDamage::NotSealed)
            ),
            "a plain payload is no sealed frame"
        );
        Ok(())
    }
}
