//! The sealed shred path: destroy the data key, commit the tombstone, burn
//! the slot.
//!
//! Split from [`super::sealed_impl`](super::sealed_impl) so neither file
//! breaches the size law: the shred's owner-thread work — the tombstone
//! framing, the length fence and the burn-set fold — lives here, while the
//! opens, reads and sweeps stay there. Both reach the same index, fence and
//! chain; only the file boundary differs.

use std::fs::File;
use std::sync::Arc;

use crate::effect::RunId;
use crate::journal::frame::SaturatingFrom;
use crate::sealed::keys::KeyId;
use crate::sealed::record::encode_tombstone;

use super::record;
use super::sealed_impl::ErasedSlot;
use super::{MAX_STORE_BYTES, StoreError, StoreLimitKind};
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
