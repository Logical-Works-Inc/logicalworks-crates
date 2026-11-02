//! The sealed half of the file journal: opening with a provider, sealing
//! appends, sealed reads, shredding, tenant erasure, TTL sweeps and chain
//! verification.
//!
//! The frame grammar, the chain check and the torn-tail discipline stay the
//! file's own ([`super::scan`], [`super::frame`](crate::journal::frame)): a
//! sealed payload is still a length, a payload and a head, so the existing
//! scan verifies sealed and shredded records byte for byte. What changes is
//! what the payload *means*: under a sealed open every event payload is an
//! envelope holding the ciphertext, the wrapped data key and the key
//! identity, the chain head commits to all of it, and reads fold a destroyed
//! key to erased. A plain payload in a sealed file, or a sealed payload in a
//! plain open, is refused rather than decoded as the other kind.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use lgwks_std::hash::{Digest, blake3};

use super::{
    Corruption, CorruptionKind, DurableAck, EffectEvent, EffectJournal, FileJournal, JournalError,
    JournalPosition, WireError, from_bytes,
};
use crate::journal::frame::{self, SaturatingFrom};
use crate::sealed::envelope::{NONCE_BYTES, open as envelope_open, seal as envelope_seal};
use crate::sealed::fold::{Unwrapped, fold_unwrap};
use crate::sealed::journal::{SealedJournalRead, ShredOutcome, write_refusal};
use crate::sealed::keys::{KeyId, KeyProvider};
use crate::sealed::record::{DecodedFrame, decode_frame, encode_record, encode_tombstone};

/// One sealed event frame the open replayed: where it sits, what seals it,
/// and whether it still opens.
pub(super) struct SealedSlot {
    /// The frame's ordinal in file order, which a tombstone names.
    pub(super) ordinal: u64,
    /// The chain sequence the frame committed at.
    pub(super) sequence: u64,
    /// The chain head the frame committed to.
    pub(super) head: Digest,
    /// The data key's identity, for destruction and for the tombstone.
    pub(super) key: KeyId,
    /// The wrapped data key, as the frame stored it: what the provider opens
    /// on every read.
    pub(super) wrapped: Vec<u8>,
    /// The seal nonce, as the frame stored it.
    pub(super) nonce: [u8; NONCE_BYTES],
    /// The ciphertext, as the frame stored it: what the chain verified.
    pub(super) ciphertext: Vec<u8>,
    /// The plaintext routing fingerprint: the BLAKE3 of the sealed event's
    /// key, which burns the slot against re-append after a shred.
    pub(super) fingerprint: [u8; 32],
    /// When the record expires on the injected clock, if it does.
    pub(super) expires_at: Option<Duration>,
    /// Whether the slot reads as erased: destroyed at open, or tombstoned.
    /// A slot that decoded but would not open reads as unreadable rather
    /// than absent: reads retry the unwrap, so a healed provider reads live
    /// again.
    pub(super) erased: bool,
}

/// The sealed half of one [`FileJournal`] handle: who wraps its keys, which
/// tenant they are minted for, and what the replay learned.
pub(super) struct SealedJournalState {
    /// Who mints, wraps, opens and destroys this file's data keys.
    pub(super) provider: Arc<dyn KeyProvider>,
    /// The tenant every key of this file is minted for. One file, one
    /// tenant namespace, so two tenants never share a data key even for the
    /// same event bytes.
    pub(super) tenant: String,
    /// One slot per sealed event frame, in file order.
    pub(super) slots: Vec<SealedSlot>,
    /// Fingerprints of shredded events: re-appending one is refused, so a
    /// restart past a shred cannot duplicate the effect the ladder forgot.
    pub(super) burned: HashSet<[u8; 32]>,
    /// Frames written through this handle, which is the next tombstone's
    /// ordinal base beside the replayed count. Advanced by every sealed
    /// append and every tombstone this handle commits.
    pub(super) frames: u64,
}

/// A sealed frame the write path just framed, before its ordinal is known.
pub(super) struct SealedPending {
    /// The data key's identity, for the slot the accept folds. Named for the
    /// identity rather than the key because the run store already calls this
    /// same concept `key_id`, and one name keeps the two sealed paths from
    /// describing one thing twice.
    pub(super) key_id: KeyId,
    /// The wrapped data key, for the slot the accept folds.
    pub(super) wrapped: Vec<u8>,
    /// The seal nonce, for the slot the accept folds.
    pub(super) nonce: [u8; NONCE_BYTES],
    /// The ciphertext, for the slot the accept folds.
    pub(super) ciphertext: Vec<u8>,
    /// The plaintext routing fingerprint, for the burn set on a later shred.
    pub(super) fingerprint: [u8; 32],
    /// When the record expires, if it does.
    pub(super) expires_at: Option<Duration>,
}

/// Whether `tenant` may name a sealed file's key namespace: non-empty,
/// short, and confined to the file-safe alphabet, so the identity the
/// chain binds is the tenant and nothing else.
pub(super) fn valid_sealed_tenant(tenant: &str) -> bool {
    !tenant.is_empty()
        && tenant.len() <= 64
        && tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The plaintext routing fingerprint of `event`: the BLAKE3 of its key's
/// wire bytes, which names the attempt without naming the tenant, the run or
/// the evidence.
fn key_fingerprint(event: &EffectEvent) -> Result<[u8; 32], JournalError> {
    let archived = event.key().to_bytes().map_err(JournalError::Encoding)?;
    Ok(*blake3(archived.as_ref()).as_bytes())
}

/// Destroy the orphan key `id` minted for an append that then refused:
/// a key no frame names must not outlive the call that minted it.
fn abandon(provider: &Arc<dyn KeyProvider>, id: &KeyId) {
    if let Err(error) = provider.destroy(id) {
        lgwks_std::trace::debug!(error = ?error, "sealed-frame: the orphan key could not be destroyed");
    }
}

/// Frame `payload` at `from`: the next sequence, the shared framing step,
/// and the chain head over the sealed bytes.
fn frame_bytes(
    payload: Vec<u8>,
    from: JournalPosition,
) -> Result<(JournalPosition, Vec<u8>), JournalError> {
    let sequence = from
        .sequence()
        .checked_add(1)
        .ok_or(JournalError::Exhausted)?;
    let (framed, head) = frame::frame_record(
        &payload,
        &from.head,
        super::MAX_FRAME_BYTES,
        |bytes| Ok(bytes.clone()),
        |_, previous, sealed| super::super::chain_over_bytes(previous, sealed),
        |_| {
            JournalError::Storage(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the sealed event exceeds this journal's frame bound",
            ))
        },
    )?;
    Ok((JournalPosition { sequence, head }, framed))
}

/// One sealed frame the scan classified: a record with its opened state, or
/// a tombstone naming a record frame.
pub(super) enum SealedClassified {
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
pub(super) enum SealedDamage {
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
pub(super) fn classify_sealed_frame(
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

/// What a sealed open carries into the scan and the replay: re-exported from
/// the file root so the scan names the same open the constructors pass.
pub(super) use super::SealedOpen;

/// Frame one event sealed: burn check, fresh key, envelope, and the shared
/// framing step over the sealed bytes.
///
/// The write path's sealed door: [`super::write`](crate::journal::file) —
/// which owns the fence, the ladder and the bounds — calls this once the
/// checks that refuse before a byte moves have passed, so the seal cannot
/// drift from the append it belongs to.
pub(super) fn frame_sealed(
    journal: &FileJournal,
    event: &EffectEvent,
    from: JournalPosition,
    expires_at: Option<Duration>,
) -> Result<(JournalPosition, Vec<u8>, SealedPending), JournalError> {
    let Some(state) = journal.sealed.as_ref() else {
        let refusal = Err(JournalError::Storage(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the sealed frame needs a sealed journal",
        )));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "frame_sealed: returning an error to the caller");
        return refusal;
    };
    let fingerprint = key_fingerprint(event)?;
    if state.burned.contains(&fingerprint) {
        let sequence = from.sequence().saturating_add(1);
        let refusal = Err(JournalError::SealedErased { sequence });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "frame_sealed: the event's slot is burned");
        return refusal;
    }
    let (key_id, data_key) = state
        .provider
        .provision(&state.tenant)
        .map_err(write_refusal)?;
    let plaintext = event
        .to_bytes()
        .map(|bytes| bytes.as_ref().to_vec())
        .map_err(JournalError::Encoding)?;
    let nonce: [u8; NONCE_BYTES] = match lgwks_std::random::bytes::<NONCE_BYTES>() {
        Ok(nonce) => nonce,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: no entropy for the seal nonce");
            abandon(&state.provider, &key_id);
            return Err(write_refusal(crate::sealed::keys::KeyError::Transient {
                reason: "no entropy for the seal nonce",
            }));
        }
    };
    let ciphertext = match envelope_seal(&data_key, &nonce, &plaintext) {
        Ok(ciphertext) => ciphertext,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: the seal step refused");
            abandon(&state.provider, &key_id);
            return Err(JournalError::Storage(std::io::Error::other(
                error.to_string(),
            )));
        }
    };
    drop(data_key);
    let wrapped = match state.provider.wrap_key(&key_id) {
        Ok(wrapped) => wrapped,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: the wrap step refused");
            abandon(&state.provider, &key_id);
            return Err(write_refusal(error));
        }
    };
    let payload = encode_record(
        state.tenant.as_str(),
        &key_id.id(),
        &wrapped,
        &nonce,
        &ciphertext,
        expires_at,
        &fingerprint,
    );
    let pending = SealedPending {
        key_id: key_id.clone(),
        wrapped,
        nonce,
        ciphertext,
        fingerprint,
        expires_at,
    };
    match frame_bytes(payload, from) {
        Ok((position, framed)) => Ok((position, framed, pending)),
        Err(error) => {
            abandon(&state.provider, &key_id);
            Err(error)
        }
    }
}

impl FileJournal {
    /// Open the journal at `path` with `provider`: every event appended
    /// through this handle is sealed under a fresh data key minted for
    /// `tenant`, and every sealed frame on disk is decrypted on read.
    ///
    /// The file, the fence, the chain and the torn-tail repair are the plain
    /// open's: a sealed payload is still a length, a payload and a head, so
    /// the scan verifies sealed and shredded frames byte for byte, and a
    /// torn tail trims the same way. What differs is the payload's meaning,
    /// so a file holding plain events is refused here rather than read as
    /// sealed, exactly as a sealed file is refused by the plain open.
    ///
    /// # Errors
    ///
    /// Whatever the plain open reports, plus [`JournalError::Corrupt`] when a
    /// committed frame is not a sealed record, a checkpoint or this handle's
    /// tombstone, and [`JournalError::Storage`] when the tenant label is not
    /// file-safe.
    #[cfg(feature = "wire")]
    pub fn open_sealed(
        path: impl AsRef<std::path::Path>,
        provider: Arc<dyn KeyProvider>,
        tenant: &str,
    ) -> Result<Self, JournalError> {
        if !valid_sealed_tenant(tenant) {
            let refusal = Err(JournalError::Storage(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the sealed tenant label is not file-safe",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "open_sealed: the tenant label is not file-safe");
            return refusal;
        }
        let journal = Self::open_impl(
            path.as_ref(),
            super::seal::OpenKind::Plain,
            Some(SealedOpen {
                provider: Arc::clone(&provider),
                tenant: tenant.to_owned(),
            }),
        )?;
        if journal.sealed.is_none() {
            let refusal = Err(JournalError::Storage(std::io::Error::other(
                "the sealed open returned a plain journal",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "open_sealed: returning an error to the caller");
            return refusal;
        }
        Ok(journal)
    }

    /// Whether this handle seals its appends: `true` for
    /// [`FileJournal::open_sealed`], `false` for every other constructor.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.sealed.is_some()
    }

    /// Append `event` sealed under a fresh data key, expiring at
    /// `expires_at` on the injected clock when `Some`.
    ///
    /// The fence, the ladder, the bounds and the flush are the plain
    /// append's: only the framing seals, and the acknowledgment still means
    /// the bytes are through the file system. The key is fresh per record —
    /// minted for this file's tenant — and the chain head covers the
    /// ciphertext together with the key identity.
    ///
    /// # Errors
    ///
    /// Every [`JournalError`] the plain append produces, plus
    /// [`JournalError::SealedErased`] when the event's key fingerprint is
    /// burned: the attempt was shredded and its slot refuses re-append.
    #[cfg(feature = "wire")]
    pub fn compare_and_append_sealed(
        &mut self,
        expected_tail: JournalPosition,
        event: &EffectEvent,
        expires_at: Option<Duration>,
    ) -> Result<DurableAck, JournalError> {
        let (position, frame, pending) =
            self.prepare_append_with_expiry(expected_tail, event, expires_at)?;
        self.write_and_sync(&frame)?;
        self.accept(event, position, frame.len(), pending);
        Ok(DurableAck::new(position, self.durability()))
    }

    /// Every sealed frame in commit order: the live events and the tombstones
    /// of the shredded records.
    ///
    /// The chain still holds every one — shredded bytes verify — while only
    /// the live events replay into the run: an erased record reads as
    /// erased, never as its value and never as absent.
    ///
    /// # Errors
    ///
    /// [`JournalError::Storage`] when the handle is plain rather than sealed.
    #[cfg(feature = "wire")]
    pub fn sealed_entries(&self) -> Result<Vec<SealedJournalRead>, JournalError> {
        let Some(state) = self.sealed.as_ref() else {
            let refusal = Err(JournalError::Storage(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "sealed reads need a sealed journal",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "sealed_entries: returning an error to the caller");
            return refusal;
        };
        let mut out = Vec::with_capacity(state.slots.len());
        for slot in &state.slots {
            if slot.erased {
                out.push(SealedJournalRead::Erased {
                    sequence: slot.sequence,
                });
                continue;
            }
            match fold_unwrap(state.provider.unwrap_key(&slot.key, &slot.wrapped)) {
                Ok(Unwrapped::Live(data_key)) => {
                    match envelope_open(&data_key, &slot.nonce, &slot.ciphertext) {
                        Ok(plaintext) => match from_bytes::<EffectEvent, WireError>(&plaintext) {
                            Ok(event) => out.push(SealedJournalRead::Live {
                                position: JournalPosition {
                                    sequence: slot.sequence,
                                    head: slot.head,
                                },
                                event: Box::new(event),
                            }),
                            Err(error) => {
                                lgwks_std::trace::debug!(
                                    slot = slot.ordinal,
                                    ?error,
                                    "sealed_entries: a committed frame does not decode"
                                );
                                return Err(JournalError::Corrupt(Box::new(Corruption::new(
                                    slot.ordinal,
                                    CorruptionKind::Undecodable,
                                ))));
                            }
                        },
                        Err(error) => {
                            lgwks_std::trace::debug!(slot = slot.ordinal, error = ?error, "sealed_entries: a live key with an unopenable envelope");
                            return Err(JournalError::Corrupt(Box::new(Corruption::new(
                                slot.ordinal,
                                CorruptionKind::Undecodable,
                            ))));
                        }
                    }
                }
                Ok(Unwrapped::Erased) => out.push(SealedJournalRead::Erased {
                    sequence: slot.sequence,
                }),
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "sealed-scan: the provider refused a sealed step");
                    return Err(write_refusal(error));
                }
            }
        }
        Ok(out)
    }

    /// Shred the sealed record at `sequence`: destroy its data key in the
    /// provider, then commit the tombstone frame.
    ///
    /// Either half surviving a crash leaves the record live or erased, never
    /// readable-without-its-managed-key: a destroyed key folds to erased
    /// even when the tombstone never landed, and a tombstone reads as erased
    /// even when the provider forgot the destruction. A second shred is
    /// [`ShredOutcome::AlreadyErased`] and touches neither provider nor file.
    ///
    /// # Errors
    ///
    /// [`JournalError::SealedUnknown`] when no sealed slot names `sequence`;
    /// [`JournalError::Storage`] when the destruction cannot be recorded — in
    /// which case the record is still live — or the tombstone cannot be
    /// committed.
    #[cfg(feature = "wire")]
    pub fn shred_sealed(&mut self, sequence: u64) -> Result<ShredOutcome, JournalError> {
        let (key, fingerprint, target) = match self.sealed.as_ref() {
            Some(state) => match state.slots.iter().find(|slot| slot.sequence == sequence) {
                Some(slot) if slot.erased => {
                    return Ok(ShredOutcome::AlreadyErased { sequence });
                }
                Some(slot) => (slot.key.clone(), slot.fingerprint, slot.ordinal),
                None => {
                    let refusal = Err(JournalError::SealedUnknown { sequence });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "shred_sealed: no sealed slot names the sequence");
                    return refusal;
                }
            },
            None => {
                let refusal = Err(JournalError::Storage(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "shredding needs a sealed journal",
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "shred_sealed: returning an error to the caller");
                return refusal;
            }
        };
        let provider = match self.sealed.as_ref() {
            Some(state) => Arc::clone(&state.provider),
            None => {
                let refusal = Err(JournalError::Storage(std::io::Error::other(
                    "the sealed open returned a plain journal",
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "shred_sealed: returning an error to the caller");
                return refusal;
            }
        };
        if let Err(error) = provider.destroy(&key) {
            lgwks_std::trace::debug!(error = ?error, "shred_sealed: the destruction could not be recorded");
            return Err(write_refusal(error));
        }
        let payload = encode_tombstone(target, &key.id());
        let tail = self.position;
        let (tomb_position, framed) = match frame_bytes(payload, tail) {
            Ok(framed) => framed,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "shred_sealed: a tombstone exceeds the frame ceiling");
                return Err(error);
            }
        };
        if let Err(error) = self.write_and_sync(&framed) {
            lgwks_std::trace::debug!(error = ?error, "shred_sealed: the tombstone could not commit; the key stays destroyed and the record reads as erased");
            if let Some(state) = self.sealed.as_mut() {
                state.burned.insert(fingerprint);
                if let Some(slot) = state
                    .slots
                    .iter_mut()
                    .find(|slot| slot.sequence == sequence)
                {
                    slot.erased = true;
                }
            }
            return Err(error);
        }
        self.position = tomb_position;
        self.disk_len = self
            .disk_len
            .saturating_add(u64::saturating_from(framed.len()));
        if let Some(state) = self.sealed.as_mut() {
            state.frames = state.frames.saturating_add(1);
            state.burned.insert(fingerprint);
            if let Some(slot) = state
                .slots
                .iter_mut()
                .find(|slot| slot.sequence == sequence)
            {
                slot.erased = true;
            }
        }
        Ok(ShredOutcome::Shredded { sequence })
    }

    /// Shred every live sealed record of this file's tenant: the tenant's
    /// right to erasure (GDPR Art. 17). The tombstones stay — the chain still
    /// verifies, and every erased record still reads as erased — so the
    /// erasure is evidenced rather than merely claimed.
    ///
    /// # Errors
    ///
    /// Whatever [`FileJournal::shred_sealed`] reports for the first record
    /// that refuses; records shredded before it stay shredded.
    /// The live sealed slots, or the refusal a plain handle gets.
    ///
    /// One definition because the tenant erasure and the TTL sweep both start
    /// from the live set before filtering to their own due set, and two
    /// spellings of the plain-handle refusal would be two chances to name a
    /// different reason for one fact.
    #[cfg(feature = "wire")]
    fn live_slots(&self, op: &'static str) -> Result<Vec<&SealedSlot>, JournalError> {
        match self.sealed.as_ref() {
            Some(state) => Ok(state.slots.iter().filter(|slot| !slot.erased).collect()),
            None => {
                let refusal = Err(JournalError::Storage(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    op,
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), op = op, "sealed: a plain journal cannot do sealed work");
                refusal
            }
        }
    }

    #[cfg(feature = "wire")]
    pub fn erase_sealed_tenant(&mut self) -> Result<u64, JournalError> {
        let live: Vec<u64> = self
            .live_slots("erasure needs a sealed journal")?
            .iter()
            .map(|slot| slot.sequence)
            .collect();
        let mut shredded = 0_u64;
        for sequence in live {
            match self.shred_sealed(sequence)? {
                ShredOutcome::Shredded { .. } => {
                    shredded = shredded.saturating_add(1);
                }
                ShredOutcome::AlreadyErased { .. } => {}
            }
        }
        Ok(shredded)
    }

    /// Shred every live sealed record expired at `now`, on the injected
    /// clock, and report how many shredded. Records without an expiry never
    /// sweep.
    ///
    /// # Errors
    ///
    /// Whatever [`FileJournal::shred_sealed`] reports for the first record
    /// that refuses; records shredded before it stay shredded.
    #[cfg(feature = "wire")]
    pub fn sweep_sealed(&mut self, now: Duration) -> Result<u64, JournalError> {
        let due: Vec<u64> = self
            .live_slots("a sweep needs a sealed journal")?
            .iter()
            .filter(|slot| slot.expires_at.is_some_and(|at| at <= now))
            .map(|slot| slot.sequence)
            .collect();
        let mut shredded = 0_u64;
        for sequence in due {
            match self.shred_sealed(sequence)? {
                ShredOutcome::Shredded { .. } => {
                    shredded = shredded.saturating_add(1);
                }
                ShredOutcome::AlreadyErased { .. } => {}
            }
        }
        Ok(shredded)
    }

    /// Verify the chain over the sealed file: every head recomputed from the
    /// bytes on disk, tombstones included. A shredded record verifies as
    /// present-and-erased — its ciphertext still authenticates — which is the
    /// whole of the erasure proof.
    ///
    /// # Errors
    ///
    /// [`JournalError::Corrupt`] when any frame does not follow or the file
    /// ends mid-frame; [`JournalError::Storage`] when the file cannot be
    /// reread or the handle is plain rather than sealed.
    #[cfg(feature = "wire")]
    pub fn verify_sealed_chain(&self) -> Result<(), JournalError> {
        let Some(state) = self.sealed.as_ref() else {
            let refusal = Err(JournalError::Storage(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "verification needs a sealed journal",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "verify_sealed_chain: returning an error to the caller");
            return refusal;
        };
        let file = File::open(&self.path).map_err(JournalError::Storage)?;
        let mut reader = BufReader::new(file);
        let ctx = SealedOpen {
            provider: Arc::clone(&state.provider),
            tenant: state.tenant.clone(),
        };
        let scanned = super::scan::scan(&mut reader, super::MAX_JOURNAL_EVENTS, Some(&ctx))?;
        let at = u64::saturating_from(scanned.entries.len());
        match scanned.stop {
            super::scan::ScanStop::Complete(len) => {
                if len != self.disk_len {
                    lgwks_std::trace::debug!(
                        len,
                        "verify_sealed_chain: the file holds a different acknowledged length than the handle"
                    );
                    return Err(JournalError::Corrupt(Box::new(Corruption::new(
                        at,
                        CorruptionKind::Framed,
                    ))));
                }
            }
            super::scan::ScanStop::Torn(offset) => {
                lgwks_std::trace::debug!(offset, "verify_sealed_chain: the file ends mid-frame");
                return Err(JournalError::Corrupt(Box::new(Corruption::new(
                    at,
                    CorruptionKind::Framed,
                ))));
            }
            super::scan::ScanStop::AmbiguousTail { offset } => {
                lgwks_std::trace::debug!(
                    offset,
                    "verify_sealed_chain: the file ends inside a declared frame"
                );
                return Err(JournalError::Corrupt(Box::new(Corruption::new(
                    at,
                    CorruptionKind::Framed,
                ))));
            }
        }
        if scanned.tail != self.tail() {
            lgwks_std::trace::debug!(
                at,
                "verify_sealed_chain: the file holds a different tail than the handle"
            );
            return Err(JournalError::Corrupt(Box::new(Corruption::new(
                at,
                CorruptionKind::Framed,
            ))));
        }
        Ok(())
    }
}
