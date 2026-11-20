//! The sealed half of the run store: opening sealed, sealed reads, shredding,
//! tenant erasure, TTL sweeps and chain verification.
//!
//! The frame grammar, the chain check and the torn-tail discipline stay the
//! store's own ([`super::record`], [`super::frame`](crate::journal::frame)):
//! a sealed payload is still a length, a payload and a head, so the replay
//! verifies sealed and shredded records byte for byte. What changes is what
//! the payload *means*: under a sealed open every record payload is an
//! envelope holding the ciphertext, the wrapped data key and the key
//! identity, the chain head commits to all of it, and reads fold a destroyed
//! key to erased. A plain payload in a sealed file, or a sealed payload in a
//! plain open, is refused rather than decoded as the other kind.

use std::fs::File;
use std::sync::Arc;
use std::time::Duration;

use crate::effect::RunId;
use crate::script::StepKey;
use crate::script::run_store::{Appended, RunRecords, StoredValue};
use crate::sealed::fold::{Unwrapped, fold_unwrap};
use crate::sealed::keys::{KeyId, KeyProvider};
use lgwks_std::hash::Digest;

use super::sealed_read::{SealedStepRead, ShredOutcome};
use super::sealed_shred::{ShredJob, provider_destroy, shred_on_owner};
use super::{RunStore, StoreError, StoreInner, record};

/// Refuse a foreign tenant's sealed read, logging the refusal where it is made.
///
/// One definition because the read and the shred both refuse a foreign tenant
/// holding a destroyed key, and two spellings would be two chances to name a
/// different owner for one event.
fn foreign_erased<T>(owner: String, asked: String) -> Result<T, StoreError> {
    lgwks_std::trace::debug!(
        owner = owner.as_str(),
        asked = asked.as_str(),
        "sealed: a foreign tenant reads a destroyed key"
    );
    Err(StoreError::ForeignTenant { owner, asked })
}

/// Refuse an unknown sealed run, logging the refusal where it is made.
///
/// One definition because the read and the shred both refuse a run the index
/// never folded, and two spellings would be two chances to name a different
/// run for one refusal.
fn unknown_run<T>(run: RunId, tenant: String) -> Result<T, StoreError> {
    lgwks_std::trace::debug!(
        run = run.id().to_hex(),
        tenant = tenant.as_str(),
        "sealed: no sealed run answers this tenant"
    );
    Err(StoreError::UnknownRun {
        run: run.id().to_hex(),
        tenant,
    })
}

/// The provider this sealed handle seals with, or the refusal a plain handle
/// gets.
///
/// One definition because the read, the shred, the erasure, the sweep and
/// the verification all refuse a plain handle with the same answer, and five
/// spellings would be five chances to name a different reason for one fact.
/// The `what` names the operation the caller attempted, which is what tells
/// five identical refusals apart in the log.
pub(super) fn sealed_provider(
    inner: &StoreInner,
    what: &'static str,
) -> Result<Arc<dyn KeyProvider>, StoreError> {
    match inner.sealed.as_ref() {
        Some(provider) => Ok(Arc::clone(provider)),
        None => {
            let refusal = Err(StoreError::Storage {
                cause: std::io::Error::new(std::io::ErrorKind::InvalidInput, what),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), op = what, "sealed: a plain handle cannot do sealed work");
            refusal
        }
    }
}

/// What a sealed open carries into the replay: who wraps this file's data
/// keys. The run store names no tenant at open — every record carries its
/// own — so only the provider travels.
#[cfg(feature = "sealed")]
pub(crate) struct SealedStoreOpen {
    /// Who mints, wraps, opens and destroys this file's data keys.
    pub(crate) provider: Arc<dyn crate::sealed::keys::KeyProvider>,
}

/// What seals one step: the envelope material the frame stored, the digest
/// the duplicate check compares, and the frame ordinal a tombstone names.
#[cfg(feature = "sealed")]
#[derive(Debug, Clone)]
pub(crate) struct SealedStep {
    /// The data key's identity, for destruction and for the tombstone.
    pub(crate) key_id: crate::sealed::keys::KeyId,
    /// The wrapped data key, as the frame stored it.
    pub(crate) wrapped: Vec<u8>,
    /// The seal nonce, as the frame stored it.
    pub(crate) nonce: [u8; 12],
    /// The ciphertext, as the frame stored it: what the chain verified.
    pub(crate) ciphertext: Vec<u8>,
    /// The BLAKE3 of the plaintext value: the duplicate check compares this
    /// instead of the value, so two identical appends still answer
    /// `AlreadyRecorded` without the index holding any secret.
    pub(crate) digest: [u8; 32],
    /// When the record expires on the injected clock, if it does.
    pub(crate) expires_at: Option<Duration>,
    /// The frame's ordinal in file order, which a tombstone names.
    pub(crate) ordinal: u64,
}

/// What a sealed append carries onto the storage owner's thread: who wraps
/// this file's data keys and when the record expires.
#[cfg(feature = "sealed")]
#[derive(Clone)]
pub(crate) struct SealedWrite {
    /// Who mints, wraps, opens and destroys this file's data keys.
    pub(crate) provider: Arc<dyn crate::sealed::keys::KeyProvider>,
    /// When the record expires on the injected clock, if it does.
    pub(crate) expires_at: Option<Duration>,
}

/// One sealed append as a caller names it: the six record facts plus the
/// expiry the TTL sweep shreds.
///
/// Public so the estate journey and the integration tests can seal through
/// the run store's own surface, rather than reaching into the script
/// module's staging. The trait's append seals with no expiry; this names one
/// on the injected clock.
#[cfg(feature = "sealed")]
#[derive(Debug)]
#[non_exhaustive]
pub struct SealedAppend<'a> {
    /// The tenant the run belongs to.
    pub(crate) tenant: &'a str,
    /// The run the record is for.
    pub(crate) run: RunId,
    /// The step key the record is filed under.
    pub(crate) key: StepKey,
    /// The step's path, for attribution.
    pub(crate) path: &'a str,
    /// The definition the value is recorded under.
    pub(crate) definition: &'a crate::task::definition::DefinitionIdentity,
    /// The archived value.
    pub(crate) bytes: Vec<u8>,
    /// When the record expires on the injected clock, if it does.
    pub(crate) expires_at: Option<Duration>,
}

#[cfg(feature = "sealed")]
impl<'a> SealedAppend<'a> {
    /// Name one sealed append: the six record facts and the expiry the TTL
    /// sweep shreds.
    #[must_use]
    pub fn new(
        tenant: &'a str,
        run: RunId,
        key: StepKey,
        path: &'a str,
        definition: &'a crate::task::definition::DefinitionIdentity,
        bytes: Vec<u8>,
        expires_at: Option<Duration>,
    ) -> Self {
        Self {
            tenant,
            run,
            key,
            path,
            definition,
            bytes,
            expires_at,
        }
    }
}

/// A shredded step the index remembers: the burn set's entry.
/// Filed by run and step key hex beside the live runs rather than in them,
/// so a fully erased run reads as unwritten — compatible with a fresh
/// resume — while any re-append under a burned slot is still refused.
#[cfg(feature = "sealed")]
#[derive(Debug, Clone)]
pub(crate) struct ErasedSlot {
    /// The destroyed data key's identity.
    pub(crate) key_id: KeyId,
    /// The tenant that minted the run, for the ownership answer.
    pub(crate) tenant: String,
    /// When the record expired, as the frame stored it.
    pub(crate) expires_at: Option<Duration>,
}

/// The tombstone read of a burned slot, logged with its key's tenant and expiry.
fn erased_read(erased: &ErasedSlot, run: RunId, key: StepKey) -> SealedStepRead {
    lgwks_std::trace::debug!(key_tenant = erased.key_id.tenant(), expires_at = ?erased.expires_at, "read_sealed: a shredded step reads as erased");
    SealedStepRead::Erased { run, key }
}

/// What a sealed step's envelope opened to.
enum Opened {
    /// The key is live and the envelope held this record.
    Live(Box<record::Stored>),
    /// The provider folded the read to erased.
    Erased,
}

/// Open a sealed step's envelope, or report the provider folded it to erased.
/// One definition for the read, the shred and the approval check, so a torn
/// frame gets one answer; `op` names the caller in every event.
fn open_stored(
    op: &'static str,
    provider: &Arc<dyn KeyProvider>,
    sealed: &SealedStep,
) -> Result<Opened, StoreError> {
    use crate::sealed::envelope::open as envelope_open;
    use lgwks_std::wire::{WireError, from_bytes};
    match fold_unwrap(provider.unwrap_key(&sealed.key_id, &sealed.wrapped)) {
        Ok(Unwrapped::Live(data_key)) => {
            let archived = envelope_open(&data_key, &sealed.nonce, &sealed.ciphertext).map_err(|error| {
                lgwks_std::trace::debug!(op, ordinal = sealed.ordinal, error = ?error, "sealed: a live key with an unopenable envelope");
                StoreError::Corrupt { at: sealed.ordinal }
            })?;
            from_bytes::<record::Stored, WireError>(&archived)
                .map(|stored| Opened::Live(Box::new(stored)))
                .map_err(|error| {
                    lgwks_std::trace::debug!(op, ordinal = sealed.ordinal, error = ?error, "sealed: a committed frame does not decode");
                    StoreError::Corrupt { at: sealed.ordinal }
                })
        }
        Ok(Unwrapped::Erased) => Ok(Opened::Erased),
        Err(error) => {
            lgwks_std::trace::debug!(op, error = ?error, "sealed: unwrapping the data key refused");
            Err(StoreError::SealedKey { source: error })
        }
    }
}

impl RunStore {
    /// Whether this handle seals its records: `true` for
    /// [`RunStore::open_sealed`], `false` for every other constructor.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.inner.sealed.is_some()
    }

    /// Open the store at `path` with `provider`: every record appended
    /// through this handle is sealed under a fresh data key, and every sealed
    /// frame on disk is decrypted on read.
    ///
    /// The file, the fence, the chain and the torn-tail repair are the plain
    /// open's: a sealed payload is still a length, a payload and a head, so
    /// the replay verifies sealed and shredded frames byte for byte. What
    /// differs is the payload's meaning, so a file holding plain records is
    /// refused here rather than read as sealed, exactly as a sealed file is
    /// refused by [`RunStore::open`](super::RunStore::open).
    ///
    /// # Errors
    ///
    /// As [`RunStore::open`](super::RunStore::open), plus
    /// [`StoreError::Corrupt`](super::StoreError::Corrupt) when a committed
    /// frame is not a sealed record or this handle's tombstone.
    #[cfg(feature = "sealed")]
    pub fn open_sealed(
        path: impl Into<std::path::PathBuf>,
        provider: Arc<dyn crate::sealed::keys::KeyProvider>,
    ) -> Result<Self, StoreError> {
        Self::open_impl(path.into(), false, Some(SealedStoreOpen { provider }))
    }

    /// Open a sealed store under `dir`, named by the tenant that owns it.
    ///
    /// The form the host uses, through a sealed open: the tenant is part of
    /// the file name, so two tenants pointed at one directory never share a
    /// file.
    ///
    /// # Errors
    ///
    /// As [`RunStore::open_sealed`].
    #[cfg(feature = "sealed")]
    pub fn open_in_sealed(
        dir: &std::path::Path,
        tenant: &str,
        provider: Arc<dyn crate::sealed::keys::KeyProvider>,
    ) -> Result<Self, StoreError> {
        std::fs::create_dir_all(dir).map_err(StoreError::storage)?;
        Self::open_sealed(dir.join(format!("{tenant}.runstore")), provider)
    }

    /// One sealed append: the six record facts plus the expiry the TTL sweep
    /// shreds. Public so bins and integration tests can seal without reaching
    /// past the run-store's own surface into the script module's staging.
    ///
    /// The expiry travels beside the provider rather than in the record
    /// because the trait's append has no expiry: an unsealed append seals
    /// with no expiry, and a sealed append names one.
    #[cfg(feature = "sealed")]
    pub fn append_with_expiry(&self, append: SealedAppend<'_>) -> Result<Appended, StoreError> {
        sealed_provider(&self.inner, "a sealed append needs a sealed store")?;
        let SealedAppend {
            tenant,
            run,
            key,
            path,
            definition,
            bytes,
            expires_at,
        } = append;
        let mut staged = self.owned(&RunRecords::stage(
            self, tenant, run, key, path, definition, bytes,
        ));
        if let Some(sealed) = staged.sealed.as_mut() {
            sealed.expires_at = expires_at;
        }
        self.commit(staged)
    }

    /// Read one sealed step: its live value, or the tombstone of a shredded
    /// record.
    ///
    /// There is no absent arm: an unknown step is
    /// [`StoreError::SealedUnknown`], never a miss that would let a finished
    /// step run again.
    ///
    /// # Errors
    ///
    /// [`StoreError::SealedUnknown`] when no sealed step names `key` under
    /// `run`; [`StoreError::SealedKey`] when the provider cannot answer, with
    /// a transient refusal kept retryable rather than folded;
    /// [`StoreError::Corrupt`] when the envelope does not open under its
    /// live key; [`StoreError::ForeignTenant`] when `run` belongs to another
    /// tenant.
    #[cfg(feature = "sealed")]
    pub fn read_sealed(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<SealedStepRead, StoreError> {
        self.step_readable()?;
        let key_hex = key.to_hex();
        let found = {
            let index = self.index();
            let Some(run_index) = index.runs.get(&run) else {
                if let Some(erased) = index.erased.get(&(run, key_hex)) {
                    if erased.tenant != tenant {
                        return foreign_erased(erased.tenant.clone(), tenant.to_owned());
                    }
                    return Ok(erased_read(erased, run, key));
                }
                return unknown_run(run, tenant.to_owned());
            };
            if run_index.tenant != tenant {
                let refusal = Err(StoreError::ForeignTenant {
                    owner: run_index.tenant.clone(),
                    asked: tenant.to_owned(),
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "read_sealed: returning an error to the caller");
                return refusal;
            }
            if let Some(erased) = index.erased.get(&(run, key_hex.clone())) {
                return Ok(erased_read(erased, run, key));
            }
            match run_index.steps.get(&key_hex) {
                Some(step) => match step.sealed.clone() {
                    Some(sealed) => sealed,
                    None => {
                        let refusal = Err(StoreError::Corrupt { at: 0 });
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "read_sealed: a sealed store holds a plain step");
                        return refusal;
                    }
                },
                None => {
                    lgwks_std::trace::debug!(
                        run = run.id().to_hex(),
                        "read_sealed: a live run names no sealed step"
                    );
                    return Err(StoreError::SealedUnknown {
                        run: run.id().to_hex(),
                        key: key_hex,
                    });
                }
            }
        };
        let provider = sealed_provider(&self.inner, "a sealed read needs a sealed store")?;
        Ok(match open_stored("read_sealed", &provider, &found)? {
            Opened::Live(stored) => SealedStepRead::Live {
                value: StoredValue::new(stored.path, stored.value),
            },
            Opened::Erased => SealedStepRead::Erased { run, key },
        })
    }

    /// Shred one sealed step: destroy its data key in the provider, then
    /// commit the tombstone frame.
    ///
    /// Either half surviving a crash leaves the step live or erased, never
    /// readable-without-its-managed-key: a destroyed key folds to erased
    /// even when the tombstone never landed, and a tombstone reads as erased
    /// even when the provider forgot the destruction. A second shred is
    /// [`ShredOutcome::AlreadyErased`] and touches neither provider nor file.
    ///
    /// # Errors
    ///
    /// [`StoreError::SealedUnknown`] when no sealed step names `key` under
    /// `run`; [`StoreError::ForeignTenant`] when `run` belongs to another
    /// tenant; [`StoreError::SealedKey`] when the destruction cannot be
    /// recorded or the value cannot be opened to adjust the approval count —
    /// in which case the step is still live.
    #[cfg(feature = "sealed")]
    pub fn shred_record(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<ShredOutcome, StoreError> {
        self.step_readable()?;
        self.shred_inner(tenant, run, &key.to_hex())
    }

    /// Shred one sealed step named by its key hex: the shared door the
    /// sweep, the tenant erasure and the public shred all shred through, so
    /// the destroy-then-tombstone order cannot drift between them.
    #[cfg(feature = "sealed")]
    pub(super) fn shred_inner(
        &self,
        tenant: &str,
        run: RunId,
        key_hex: &str,
    ) -> Result<ShredOutcome, StoreError> {
        use crate::script::approval::{RecordClass, classify_record};
        // The slot, the value's approval class and the provider, each read
        // before the destruction: nothing here mutates the store, so a
        // refusal leaves the step live.
        let (slot, was_pending) = {
            let index = self.index();
            let Some(run_index) = index.runs.get(&run) else {
                if let Some(erased) = index.erased.get(&(run, key_hex.to_owned())) {
                    if erased.tenant != tenant {
                        return foreign_erased(erased.tenant.clone(), tenant.to_owned());
                    }
                    return Self::already_erased(run, key_hex);
                }
                return unknown_run(run, tenant.to_owned());
            };
            if run_index.tenant != tenant {
                lgwks_std::trace::debug!(
                    owner = run_index.tenant.as_str(),
                    tenant = tenant,
                    "shred_record: a foreign tenant shreds another tenant's step"
                );
                return Err(StoreError::ForeignTenant {
                    owner: run_index.tenant.clone(),
                    asked: tenant.to_owned(),
                });
            }
            if index.erased.contains_key(&(run, key_hex.to_owned())) {
                return Self::already_erased(run, key_hex);
            }
            let Some(step) = run_index.steps.get(key_hex) else {
                lgwks_std::trace::debug!(
                    run = run.id().to_hex(),
                    "shred_record: a live run names no sealed step"
                );
                return Err(StoreError::SealedUnknown {
                    run: run.id().to_hex(),
                    key: key_hex.to_owned(),
                });
            };
            let Some(sealed) = step.sealed.clone() else {
                lgwks_std::trace::debug!("shred_record: a sealed shred meets a plain step");
                return Err(StoreError::Corrupt { at: 0 });
            };
            let provider = sealed_provider(&self.inner, "shredding needs a sealed store")?;
            let value = match open_stored("shred_record", &provider, &sealed)? {
                Opened::Live(stored) => stored.value,
                Opened::Erased => return Self::already_erased(run, key_hex),
            };
            (
                sealed,
                matches!(classify_record(&value), RecordClass::Pending(_)),
            )
        };
        provider_destroy(&self.inner, &slot.key_id)?;
        let job = ShredJob {
            run,
            key_hex: key_hex.to_owned(),
            key_id: slot.key_id,
            ordinal: slot.ordinal,
            was_pending,
            tenant: tenant.to_owned(),
        };
        self.inner
            .owner
            .submit(move |file, index| shred_on_owner(file, index, job))
            .map_err(StoreError::from)?;
        Ok(ShredOutcome::Shredded {
            run,
            key: Self::key_of(key_hex)?,
        })
    }

    /// The answer for a step that is already a tombstone: nothing destroyed,
    /// nothing written.
    fn already_erased(run: RunId, key_hex: &str) -> Result<ShredOutcome, StoreError> {
        Ok(ShredOutcome::AlreadyErased {
            run,
            key: Self::key_of(key_hex)?,
        })
    }

    /// Rebuild a [`StepKey`] from its hex: the digest the store keys by.
    ///
    /// The hex came from [`StepKey::to_hex`], so a shape that does not parse
    /// is a file this store did not write and is refused as corrupt rather
    /// than guessed.
    fn key_of(key_hex: &str) -> Result<StepKey, StoreError> {
        let digest = Digest::from_hex(key_hex).map_err(|error| {
            lgwks_std::trace::debug!(key_hex, error = ?error, "shred_record: a key hex does not parse");
            StoreError::Corrupt { at: 0 }
        })?;
        Ok(StepKey::from_digest(digest))
    }

    /// Verify the chain over the sealed file: every head recomputed from the
    /// bytes on disk, tombstones included. A shredded step verifies as
    /// present-and-erased, which is the whole of the erasure proof.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] when any frame does not follow or the file
    /// ends mid-frame; [`StoreError::Storage`] when the file cannot be
    /// reread or the handle is plain rather than sealed.
    #[cfg(feature = "sealed")]
    pub fn verify_sealed_chain(&self) -> Result<(), StoreError> {
        let provider = sealed_provider(&self.inner, "verification needs a sealed store")?;
        let mut file = File::open(&self.inner.path).map_err(StoreError::storage)?;
        let replayed = record::replay_sealed(&mut file, &provider)?;
        let index = self.index();
        if replayed.committed != index.committed || replayed.tail != index.tail {
            lgwks_std::trace::debug!(
                "verify_sealed_chain: the file holds a different tail than the handle"
            );
            return Err(StoreError::Corrupt { at: 0 });
        }
        Ok(())
    }
}

/// Open a sealed step's envelope to its archived value bytes: the live
/// answer, or `None` when the provider folds the read to erased.
///
/// Only a destroyed key is `None`. Every refusal, transient included, is the
/// typed [`StoreError::SealedKey`] (or `Corrupt` for an envelope a live key
/// cannot open), so a listing never mistakes "cannot answer now" for "gone"
/// and drops a parked approval while the key service has a fault (#1085).
///
/// # Errors
///
/// [`StoreError::SealedKey`] when the provider refuses the unwrap, and
/// [`StoreError::Corrupt`] when a live key does not open the frame.
#[cfg(feature = "sealed")]
pub(super) fn pending_sealed_value(
    provider: &Arc<dyn crate::sealed::keys::KeyProvider>,
    sealed: &SealedStep,
) -> Result<Option<Vec<u8>>, StoreError> {
    match open_stored("pending_scan", provider, sealed)? {
        Opened::Live(stored) => Ok(Some(stored.value)),
        Opened::Erased => Ok(None),
    }
}

/// The sealed duplicate answer for a live step: compare the digest, never
/// the value, so the check retains no secret.
#[cfg(feature = "sealed")]
pub(super) fn sealed_duplicate(
    sealed: &SealedStep,
    value: &[u8],
) -> crate::script::run_store::Appended {
    let digest = record::value_digest(value);
    if sealed.digest == digest {
        Appended::AlreadyRecorded
    } else {
        Appended::Conflicting
    }
}

/// Refuse a re-append under a burned slot: the tombstone still names the
/// frame, so the slot stays refused past every reopen.
#[cfg(feature = "sealed")]
pub(super) fn refuse_if_burned(
    index: &super::Index,
    run: RunId,
    key_hex: &str,
) -> Option<StoreError> {
    index
        .erased
        .contains_key(&(run, key_hex.to_owned()))
        .then(|| StoreError::SealedErased {
            run: run.id().to_hex(),
            key: key_hex.to_owned(),
        })
}

/// Frame `stored` over `previous`: seal under a fresh data key when `write`
/// names one, else frame plain. The chain closure is the file's own either
/// way, so a sealed payload and a plain payload verify byte for byte.
#[cfg(feature = "sealed")]
pub(super) fn frame_stored(
    stored: &record::Stored,
    previous: &Digest,
    write: Option<&SealedWrite>,
) -> Result<(Vec<u8>, Digest, Option<record::SealedFramed>), StoreError> {
    match write {
        Some(write) => {
            let (framed, head, slot) = record::frame_sealed(stored, previous, write)?;
            Ok((framed, head, Some(slot)))
        }
        None => {
            let (framed, head) = record::frame(stored, previous)?;
            Ok((framed, head, None))
        }
    }
}

/// The index entry for a settled write: envelope material for a sealed
/// record, the value for a plain one. The digest travels for the duplicate
/// check; the plaintext never enters the index for a sealed step.
#[cfg(feature = "sealed")]
pub(super) fn held_for_settle(
    stored: &record::Stored,
    slot: Option<record::SealedFramed>,
) -> super::Held {
    match slot {
        Some(slot) => super::Held {
            path: stored.path.clone(),
            bytes: Vec::new(),
            sealed: Some(SealedStep {
                key_id: slot.key_id,
                wrapped: slot.wrapped,
                nonce: slot.nonce,
                ciphertext: slot.ciphertext,
                digest: slot.digest,
                expires_at: slot.expires_at,
                // Assigned at fold time from the frame counter, so the
                // ordinal matches the frame's file position on every reopen.
                ordinal: 0,
            }),
        },
        None => super::Held {
            path: stored.path.clone(),
            bytes: stored.value.clone(),
            sealed: None,
        },
    }
}
