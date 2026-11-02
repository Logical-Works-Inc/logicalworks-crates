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
use crate::script::run_store::StoredValue;
use crate::sealed::fold::{Unwrapped, fold_unwrap};
use crate::sealed::keys::{KeyId, KeyProvider};
use crate::sealed::runstore::{SealedStepRead, ShredOutcome};
use crate::sealed::sweep::{SweepError, SweepTarget};
use lgwks_std::hash::Digest;

use super::{RunStore, StoreError, StoreInner, record};
use crate::script::run_store::Appended;
use crate::script::run_store::RunRecords;

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
fn sealed_provider(
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

/// One ordered visit per run the index holds, by key hex.
///
/// One definition because the tenant erasure and the TTL sweep both walk the
/// runs in key order before filtering to their own steps, and two spellings
/// would be two chances to visit a different order for one index.
fn each_run(index: &super::Index, mut visit: impl FnMut(RunId, &super::RunIndex)) {
    let mut runs: Vec<RunId> = index.runs.keys().copied().collect();
    runs.sort_by_key(|run| run.id().to_hex());
    for run in runs {
        if let Some(run_index) = index.runs.get(&run) {
            visit(run, run_index);
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
///
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

    /// Stage `record` with a sealed expiry: what
    /// [`RunStore::append_with_expiry`](Self::append_with_expiry) seals.
    ///
    /// The expiry travels beside the provider rather than in the record
    /// because the trait's append has no expiry: an unsealed append seals
    /// with no expiry, and a sealed append names one.
    #[cfg(feature = "sealed")]
    fn with_expiry(
        &self,
        record: crate::script::run_store::StagedRecord<'_>,
        expires_at: Option<Duration>,
    ) -> super::Staged {
        let mut staged = self.owned(record);
        if let Some(sealed) = staged.sealed.as_mut() {
            sealed.expires_at = expires_at;
        }
        staged
    }

    /// One sealed append: the six record facts plus the expiry the TTL sweep
    /// shreds. Public so bins and integration tests can seal without reaching
    /// past the run-store's own surface into the script module's staging.
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
        let staged = self.with_expiry(
            RunRecords::stage(self, tenant, run, key, path, definition, bytes),
            expires_at,
        );
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
        use crate::sealed::keys::KeyError as ProviderRefusal;
        self.step_readable()?;
        let key_hex = key.to_hex();
        let found = {
            let index = self.index();
            let Some(run_index) = index.runs.get(&run) else {
                if let Some(erased) = index.erased.get(&(run, key_hex)) {
                    if erased.tenant != tenant {
                        return foreign_erased(erased.tenant.clone(), tenant.to_owned());
                    }
                    let _ = (erased.key_id.tenant().len(), erased.expires_at);
                    return Ok(SealedStepRead::Erased { run, key });
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
                let _ = (erased.key_id.tenant().len(), erased.expires_at);
                return Ok(SealedStepRead::Erased { run, key });
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
        match fold_unwrap(provider.unwrap_key(&found.key_id, &found.wrapped)) {
            Ok(Unwrapped::Live(data_key)) => {
                use crate::sealed::envelope::open as envelope_open;
                use lgwks_std::wire::WireError;
                use lgwks_std::wire::from_bytes as wire_from_bytes;
                let plaintext = match envelope_open(&data_key, &found.nonce, &found.ciphertext) {
                    Ok(plaintext) => plaintext,
                    Err(error) => {
                        lgwks_std::trace::debug!(ordinal = found.ordinal, error = ?error, "read_sealed: a live key with an unopenable envelope");
                        return Err(StoreError::Corrupt { at: found.ordinal });
                    }
                };
                let stored = match wire_from_bytes::<crate::task::store::Stored, WireError>(
                    &plaintext,
                ) {
                    Ok(stored) => stored,
                    Err(error) => {
                        lgwks_std::trace::debug!(ordinal = found.ordinal, error = ?error, "read_sealed: a committed frame does not decode");
                        return Err(StoreError::Corrupt { at: found.ordinal });
                    }
                };
                Ok(SealedStepRead::Live {
                    value: StoredValue::new(stored.path, stored.value),
                })
            }
            Ok(Unwrapped::Erased) => Ok(SealedStepRead::Erased { run, key }),
            Err(ProviderRefusal::Transient { reason }) => Err(StoreError::SealedKey {
                source: ProviderRefusal::Transient { reason },
            }),
            Err(other) => Err(StoreError::SealedKey { source: other }),
        }
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
    fn shred_inner(
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
            let run_index = match index.runs.get(&run) {
                Some(run_index) => run_index,
                None => {
                    if let Some(erased) = index.erased.get(&(run, key_hex.to_owned())) {
                        if erased.tenant != tenant {
                            return foreign_erased(erased.tenant.clone(), tenant.to_owned());
                        }
                        return Ok(ShredOutcome::AlreadyErased {
                            run,
                            key: Self::key_of(key_hex)?,
                        });
                    }
                    return unknown_run(run, tenant.to_owned());
                }
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
                return Ok(ShredOutcome::AlreadyErased {
                    run,
                    key: Self::key_of(key_hex)?,
                });
            }
            let step = match run_index.steps.get(key_hex) {
                Some(step) => step,
                None => {
                    lgwks_std::trace::debug!(
                        run = run.id().to_hex(),
                        "shred_record: a live run names no sealed step"
                    );
                    return Err(StoreError::SealedUnknown {
                        run: run.id().to_hex(),
                        key: key_hex.to_owned(),
                    });
                }
            };
            let sealed = match step.sealed.as_ref() {
                Some(sealed) => sealed.clone(),
                None => {
                    lgwks_std::trace::debug!("shred_record: a sealed shred meets a plain step");
                    return Err(StoreError::Corrupt { at: 0 });
                }
            };
            let provider = sealed_provider(&self.inner, "shredding needs a sealed store")?;
            let value = match fold_unwrap(provider.unwrap_key(&sealed.key_id, &sealed.wrapped)) {
                Ok(Unwrapped::Live(data_key)) => {
                    use crate::sealed::envelope::open as envelope_open;
                    match envelope_open(&data_key, &sealed.nonce, &sealed.ciphertext) {
                        Ok(archived) => {
                            use lgwks_std::wire::{WireError, from_bytes};
                            match from_bytes::<super::record::Stored, WireError>(&archived) {
                                Ok(stored) => stored.value,
                                Err(error) => {
                                    lgwks_std::trace::debug!(ordinal = sealed.ordinal, error = ?error, "shred_record: a committed frame does not decode");
                                    return Err(StoreError::Corrupt { at: sealed.ordinal });
                                }
                            }
                        }
                        Err(error) => {
                            lgwks_std::trace::debug!(ordinal = sealed.ordinal, error = ?error, "shred_record: a live key with an unopenable envelope");
                            return Err(StoreError::Corrupt { at: sealed.ordinal });
                        }
                    }
                }
                Ok(Unwrapped::Erased) => {
                    return Ok(ShredOutcome::AlreadyErased {
                        run,
                        key: Self::key_of(key_hex)?,
                    });
                }
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "shred_record: unwrapping the data key refused the shred");
                    return Err(StoreError::SealedKey { source: error });
                }
            };
            let was_pending = matches!(classify_record(&value), RecordClass::Pending(_));
            (sealed, was_pending)
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

    /// Rebuild a [`StepKey`] from its hex: the digest the store keys by.
    ///
    /// The hex came from [`StepKey::to_hex`], so a shape that does not parse
    /// is a file this store did not write and is refused as corrupt rather
    /// than guessed.
    fn key_of(key_hex: &str) -> Result<StepKey, StoreError> {
        use lgwks_std::hash::Digest;
        let digest = Digest::from_hex(key_hex).map_err(|error| {
            lgwks_std::trace::debug!(key_hex, error = ?error, "shred_record: a key hex does not parse");
            StoreError::Corrupt { at: 0 }
        })?;
        Ok(StepKey::from_digest(digest))
    }

    /// Shred every live sealed step of `tenant` and report how many shredded.
    ///
    /// The tenant's right to erasure (GDPR Art. 17): each step shreds through
    /// the shared shred door, so the destroy-then-tombstone order cannot
    /// drift. Records shredded before a refusal stay shredded.
    ///
    /// # Errors
    ///
    /// Whatever [`RunStore::shred_record`] reports for the first step that
    /// refuses; [`StoreError::Storage`] when the handle is plain rather than
    /// sealed.
    #[cfg(feature = "sealed")]
    pub fn erase_tenant_records(&self, tenant: &str) -> Result<u64, StoreError> {
        sealed_provider(&self.inner, "erasure needs a sealed store")?;
        let live: Vec<(RunId, String)> = {
            let index = self.index();
            let mut out = Vec::new();
            each_run(&index, |run, run_index| {
                if run_index.tenant != tenant {
                    return;
                }
                let mut keys: Vec<&String> = run_index.steps.keys().collect();
                keys.sort();
                for key_hex in keys {
                    if index.erased.contains_key(&(run, key_hex.clone())) {
                        continue;
                    }
                    out.push((run, key_hex.clone()));
                }
            });
            out
        };
        let mut shredded = 0_u64;
        for (run, key_hex) in live {
            match self.shred_inner(tenant, run, &key_hex)? {
                ShredOutcome::Shredded { .. } => {
                    shredded = shredded.saturating_add(1);
                }
                ShredOutcome::AlreadyErased { .. } => {}
            }
        }
        Ok(shredded)
    }

    /// Shred every live sealed step expired at `now`, on the injected clock,
    /// and report how many shredded. Steps without an expiry never sweep.
    ///
    /// # Errors
    ///
    /// Whatever [`RunStore::shred_record`] reports for the first step that
    /// refuses; [`StoreError::Storage`] when the handle is plain rather than
    /// sealed.
    #[cfg(feature = "sealed")]
    pub fn sweep_once(&self, now: Duration) -> Result<u64, StoreError> {
        sealed_provider(&self.inner, "a sweep needs a sealed store")?;
        let due: Vec<(RunId, String, String)> = {
            let index = self.index();
            let mut out = Vec::new();
            each_run(&index, |run, run_index| {
                let tenant = run_index.tenant.clone();
                let mut keys: Vec<&String> = run_index.steps.keys().collect();
                keys.sort();
                for key_hex in keys {
                    if index.erased.contains_key(&(run, key_hex.clone())) {
                        continue;
                    }
                    let Some(step) = run_index.steps.get(key_hex) else {
                        continue;
                    };
                    let Some(sealed) = step.sealed.as_ref() else {
                        continue;
                    };
                    if sealed.expires_at.is_some_and(|at| at <= now) {
                        out.push((run, key_hex.clone(), tenant.clone()));
                    }
                }
            });
            out
        };
        let mut shredded = 0_u64;
        for (run, key_hex, tenant) in due {
            match self.shred_inner(&tenant, run, &key_hex)? {
                ShredOutcome::Shredded { .. } => {
                    shredded = shredded.saturating_add(1);
                }
                ShredOutcome::AlreadyErased { .. } => {}
            }
        }
        Ok(shredded)
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

impl SweepTarget for RunStore {
    type Error = SweepError;

    fn sweep_once(&self, now: Duration) -> Result<u64, Self::Error> {
        RunStore::sweep_once(self, now).map_err(SweepError::from)
    }
}

use super::sealed_shred::{ShredJob, provider_destroy, shred_on_owner};

/// Open a sealed step's envelope to its archived value bytes: the live
/// answer, or `None` when the provider folds the read to erased.
///
/// `None` covers the destroyed key and every refusal: a torn provider
/// answer never becomes bytes, and a missing frame never becomes a value.
/// The caller classifies the bytes; this names only whether they open.
#[cfg(feature = "sealed")]
pub(super) fn pending_sealed_value(
    provider: &Arc<dyn crate::sealed::keys::KeyProvider>,
    sealed: &SealedStep,
) -> Option<Vec<u8>> {
    let value = match fold_unwrap(provider.unwrap_key(&sealed.key_id, &sealed.wrapped)) {
        Ok(Unwrapped::Live(data_key)) => {
            match crate::sealed::envelope::open(&data_key, &sealed.nonce, &sealed.ciphertext) {
                Ok(archived) => {
                    use lgwks_std::wire::{WireError, from_bytes};
                    match from_bytes::<record::Stored, WireError>(&archived) {
                        Ok(stored) => stored.value,
                        Err(_) => return None,
                    }
                }
                Err(_) => return None,
            }
        }
        Ok(Unwrapped::Erased) | Err(_) => return None,
    };
    Some(value)
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
    if index.erased.contains_key(&(run, key_hex.to_owned())) {
        Some(StoreError::SealedErased {
            run: run.id().to_hex(),
            key: key_hex.to_owned(),
        })
    } else {
        None
    }
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
