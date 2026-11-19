//! The sealed shred path: destroy the data key, commit the tombstone, burn
//! the slot.
//!
//! Split from [`super::sealed_impl`](super::sealed_impl) so neither file
//! breaches the size law: the shred's owner-thread work — the tombstone
//! framing, the length fence and the burn-set fold — lives here, together with
//! the two callers that shred many steps through the one shred door, the
//! tenant erasure and the TTL sweep. The opens, reads and the single shred
//! stay there. Both reach the same index, fence and chain; only the file
//! boundary differs.

use std::fs::File;
use std::sync::Arc;
use std::time::Duration;

use crate::effect::RunId;
use crate::journal::frame::SaturatingFrom;
use crate::sealed::keys::KeyId;
use crate::sealed::record::encode_tombstone;
use crate::sealed::sweep::SweepTarget;

use super::record;
use super::sealed_impl::{ErasedSlot, sealed_provider};
use super::sealed_read::ShredOutcome;
use super::{MAX_STORE_BYTES, RunStore, StoreError, StoreLimitKind};
use crate::journal::owner::{self, Stage};
use crate::script::run_store::Appended;

/// Destroy `id` in the provider behind `inner`, mapping the refusal onto the
/// store's vocabulary.
///
/// A second destroy is `Ok`: the provider's destruction log makes shredding
/// idempotent, so a retry after a torn tombstone does not refuse.
pub(super) fn provider_destroy(inner: &super::StoreInner, id: &KeyId) -> Result<(), StoreError> {
    let provider = match inner.sealed.as_ref() {
        Some(provider) => Arc::clone(provider),
        None => {
            let refusal = Err(StoreError::Storage {
                cause: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "shredding needs a sealed store",
                ),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider_destroy: returning an error to the caller");
            return refusal;
        }
    };
    provider
        .destroy(id)
        .map_err(|error| StoreError::SealedKey { source: error })
}

/// One shred's owner-thread work: the tombstone to commit and the slot to burn.
///
/// Grouped because the owner thread takes one job, not seven facts: the run,
/// the key and the tenant name the slot, the ordinal and key identity name
/// the tombstone, and the pending flag adjusts the approval count.
pub(super) struct ShredJob {
    /// The run the shredded step belongs to.
    pub(super) run: RunId,
    /// The shredded step's key, as hex.
    pub(super) key_hex: String,
    /// The destroyed data key's identity, for the tombstone.
    pub(super) key_id: KeyId,
    /// The shredded record's frame ordinal, for the tombstone.
    pub(super) ordinal: u64,
    /// Whether the shredded value was a parked approval.
    pub(super) was_pending: bool,
    /// The tenant that minted the run, for the ownership check.
    pub(super) tenant: String,
}

/// The owner-thread half of a shred: write the tombstone and fold the burn.
///
/// Runs inside the owner's ordered step, so the check and the write it guards
/// cannot be split by another append. The destroy already happened: this only
/// commits the tombstone and moves the live step to the burn set. A step
/// already erased answers [`Appended::AlreadyRecorded`] and writes nothing,
/// which is what makes a retried shred idempotent on the file as well as in
/// the provider.
pub(super) fn shred_on_owner(
    file: &mut File,
    shared: &mut Arc<std::sync::Mutex<super::Index>>,
    job: ShredJob,
) -> std::io::Result<Stage<Appended, Arc<std::sync::Mutex<super::Index>>>> {
    let ShredJob {
        run,
        key_hex,
        key_id,
        ordinal,
        was_pending,
        tenant,
    } = job;
    use super::record::refusal;
    let mut index = owner::lock(shared);
    if index.erased.contains_key(&(run, key_hex.clone())) {
        return Ok(Stage::Settled(Ok(Appended::AlreadyRecorded)));
    }
    let Some(run_index) = index.runs.get(&run) else {
        lgwks_std::trace::debug!(
            run = run.id().to_hex(),
            "shred_on_owner: the index holds no such run"
        );
        return Err(refusal(StoreError::SealedUnknown {
            run: run.id().to_hex(),
            key: key_hex,
        }));
    };
    if run_index.tenant != tenant {
        lgwks_std::trace::debug!(
            owner = run_index.tenant.as_str(),
            tenant = tenant,
            "shred_on_owner: the step belongs to another tenant"
        );
        return Err(refusal(StoreError::ForeignTenant {
            owner: run_index.tenant.clone(),
            asked: tenant,
        }));
    }
    if !run_index.steps.contains_key(&key_hex) {
        lgwks_std::trace::debug!(
            run = run.id().to_hex(),
            "shred_on_owner: the run holds no such step"
        );
        let refusal = Err(refusal(StoreError::SealedUnknown {
            run: run.id().to_hex(),
            key: key_hex,
        }));
        return refusal;
    }
    let payload = encode_tombstone(ordinal, &key_id.id());
    let previous = record::tail_head(&index);
    let (framed, head) = crate::journal::frame::frame_record(
        &payload,
        &previous,
        super::MAX_RECORD_BYTES,
        |bytes| Ok(bytes.clone()),
        |_, prior, sealed| crate::journal::chain_over_bytes(prior, sealed),
        |len| StoreError::Limit {
            kind: StoreLimitKind::RecordBytes,
            requested: u64::saturating_from(len),
            limit: u64::saturating_from(super::MAX_RECORD_BYTES),
        },
    )
    .map_err(refusal)?;
    let staged = u64::saturating_from(framed.len());
    let next = index.committed.checked_add(staged).ok_or_else(|| {
        refusal(StoreError::Limit {
            kind: StoreLimitKind::StoreBytes,
            requested: u64::MAX,
            limit: MAX_STORE_BYTES,
        })
    })?;
    if next > MAX_STORE_BYTES {
        lgwks_std::trace::debug!(
            next = next,
            "shred_on_owner: the tombstone would pass the store bound"
        );
        return Err(refusal(StoreError::Limit {
            kind: StoreLimitKind::StoreBytes,
            requested: next,
            limit: MAX_STORE_BYTES,
        }));
    }
    let on_disk = file
        .metadata()
        .map_err(StoreError::storage)
        .map_err(refusal)?
        .len();
    if on_disk != index.committed {
        lgwks_std::trace::debug!(
            ordinal = ordinal,
            "shred_on_owner: the file moved under the shred"
        );
        return Err(refusal(StoreError::Corrupt { at: ordinal }));
    }
    index.committed = next;
    index.tail = head;
    index.frames = index.frames.saturating_add(1);
    use std::io::Write as _;
    file.write_all(&framed)
        .map_err(StoreError::storage)
        .map_err(refusal)?;
    Ok(Stage::Unsynced {
        answer: Appended::Recorded,
        bytes: framed.len(),
        settle: Box::new(move |shared: &mut Arc<std::sync::Mutex<super::Index>>| {
            let mut index = owner::lock(shared);
            index.staged.remove(&(run, key_hex.clone()));
            if was_pending && let Some(held) = index.pending.get_mut(&tenant) {
                *held = held.saturating_sub(1);
            }
            let step = index
                .runs
                .get_mut(&run)
                .and_then(|run_index| run_index.steps.remove(&key_hex));
            let is_empty = index
                .runs
                .get(&run)
                .is_some_and(|run_index| run_index.steps.is_empty());
            if is_empty {
                index.runs.remove(&run);
            }
            let expires_at = step
                .as_ref()
                .and_then(|step| step.sealed.as_ref())
                .and_then(|sealed| sealed.expires_at);
            let tenant_for_burn = match index.runs.get(&run) {
                Some(run_index) => run_index.tenant.clone(),
                None => tenant,
            };
            let counts = index
                .erased_runs
                .entry(run)
                .or_insert_with(|| (0, tenant_for_burn.clone()));
            counts.0 = counts.0.saturating_add(1);
            index.erased.insert(
                (run, key_hex),
                ErasedSlot {
                    key_id,
                    tenant: tenant_for_burn,
                    expires_at,
                },
            );
        }),
    })
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

impl RunStore {
    /// Shred every live step `wanted` picks, in run then key order, and report
    /// how many shredded now.
    ///
    /// One definition because the tenant erasure and the TTL sweep differ only
    /// in which steps they want; each step shreds through the shared shred
    /// door, so the destroy-then-tombstone order cannot drift. Steps shredded
    /// before a refusal stay shredded.
    #[cfg(feature = "sealed")]
    fn shred_matching(
        &self,
        mut wanted: impl FnMut(&super::RunIndex, &super::Held) -> bool,
    ) -> Result<u64, StoreError> {
        let picked: Vec<(RunId, String, String)> = {
            let index = self.index();
            let mut out = Vec::new();
            each_run(&index, |run, run_index| {
                let mut steps: Vec<(&String, &super::Held)> = run_index.steps.iter().collect();
                steps.sort_by_key(|entry| entry.0);
                for (key_hex, step) in steps {
                    if !index.erased.contains_key(&(run, key_hex.clone()))
                        && wanted(run_index, step)
                    {
                        out.push((run, key_hex.clone(), run_index.tenant.clone()));
                    }
                }
            });
            out
        };
        let mut shredded = 0_u64;
        for (run, key_hex, tenant) in picked {
            if let ShredOutcome::Shredded { .. } = self.shred_inner(&tenant, run, &key_hex)? {
                shredded = shredded.saturating_add(1);
            }
        }
        Ok(shredded)
    }

    /// Shred every live sealed step of `tenant` and report how many shredded.
    ///
    /// The tenant's right to erasure (GDPR Art. 17): each step shreds through
    /// the shared shred door, so the destroy-then-tombstone order cannot
    /// drift. Records shredded before a refusal stay shredded, and another
    /// tenant's records are never picked.
    ///
    /// # Errors
    ///
    /// Whatever [`RunStore::shred_record`] reports for the first step that
    /// refuses; [`StoreError::Storage`] when the handle is plain rather than
    /// sealed.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use lgwks_bot::effect::RunId;
    /// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
    /// use lgwks_bot::script::{Scope, Tenant};
    /// use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend, SealedStepRead};
    ///
    /// let dir = std::env::temp_dir().join("lgwks-doc-erase-tenant-records");
    /// std::fs::create_dir_all(&dir)?;
    /// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
    /// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
    /// let definition = DefinitionIdentity::new("doc", 1, lgwks_std::hash::blake3(b"doc"), 1);
    /// let run_a = RunId::from_hex("00000000000000000000000000000002")?;
    /// let run_b = RunId::from_hex("00000000000000000000000000000003")?;
    /// let acme = Scope::root(Tenant::new("acme")?).enter("note")?.key();
    /// let globex = Scope::root(Tenant::new("globex")?).enter("note")?.key();
    /// store.append_with_expiry(SealedAppend::new(
    ///     "acme", run_a, acme, "note", &definition, b"a".to_vec(), None,
    /// ))?;
    /// store.append_with_expiry(SealedAppend::new(
    ///     "globex", run_b, globex, "note", &definition, b"b".to_vec(), None,
    /// ))?;
    ///
    /// // One of acme's steps shreds; globex's step is not acme's to lose.
    /// assert_eq!(store.erase_tenant_records("acme")?, 1);
    /// assert!(matches!(store.read_sealed("acme", run_a, acme)?, SealedStepRead::Erased { .. }));
    /// assert!(matches!(store.read_sealed("globex", run_b, globex)?, SealedStepRead::Live { .. }));
    /// drop(store);
    /// std::fs::remove_dir_all(&dir)?;
    /// # Ok(()) }
    /// ```
    #[cfg(feature = "sealed")]
    pub fn erase_tenant_records(&self, tenant: &str) -> Result<u64, StoreError> {
        sealed_provider(&self.inner, "erasure needs a sealed store")?;
        self.shred_matching(|run_index, _| run_index.tenant == tenant)
    }

    /// Shred every live sealed step expired at `now`, on the injected clock,
    /// and report how many shredded. Steps without an expiry never sweep.
    ///
    /// # Errors
    ///
    /// Whatever [`RunStore::shred_record`] reports for the first step that
    /// refuses; [`StoreError::Storage`] when the handle is plain rather than
    /// sealed.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use lgwks_bot::effect::RunId;
    /// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
    /// use lgwks_bot::script::{Scope, Tenant};
    /// use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend, SealedStepRead};
    ///
    /// let dir = std::env::temp_dir().join("lgwks-doc-sweep-once");
    /// std::fs::create_dir_all(&dir)?;
    /// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
    /// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
    /// let definition = DefinitionIdentity::new("doc", 1, lgwks_std::hash::blake3(b"doc"), 1);
    /// let run = RunId::from_hex("00000000000000000000000000000002")?;
    /// let key = Scope::root(Tenant::new("acme")?).enter("session")?.key();
    /// store.append_with_expiry(SealedAppend::new(
    ///     "acme", run, key, "session", &definition, b"token".to_vec(),
    ///     Some(Duration::from_secs(5)),
    /// ))?;
    ///
    /// // The clock is the caller's: one second short of the expiry shreds
    /// // nothing, the expiry itself shreds the one record.
    /// assert_eq!(store.sweep_once(Duration::from_secs(4))?, 0);
    /// assert_eq!(store.sweep_once(Duration::from_secs(5))?, 1);
    /// assert!(matches!(store.read_sealed("acme", run, key)?, SealedStepRead::Erased { .. }));
    /// drop(store);
    /// std::fs::remove_dir_all(&dir)?;
    /// # Ok(()) }
    /// ```
    #[cfg(feature = "sealed")]
    pub fn sweep_once(&self, now: Duration) -> Result<u64, StoreError> {
        sealed_provider(&self.inner, "a sweep needs a sealed store")?;
        self.shred_matching(|_, step| {
            step.sealed
                .as_ref()
                .is_some_and(|sealed| sealed.expires_at.is_some_and(|at| at <= now))
        })
    }
}

impl SweepTarget for RunStore {
    type Error = StoreError;

    fn sweep_once(&self, now: Duration) -> Result<u64, Self::Error> {
        RunStore::sweep_once(self, now)
    }
}
