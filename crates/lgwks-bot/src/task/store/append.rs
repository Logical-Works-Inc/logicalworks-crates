//! The ordered append and stage/settle logic for the durable run store.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::journal::frame::SaturatingFrom;
use crate::journal::owner::{self, Stage};
use crate::script::StepKey;
use crate::script::run_store::Appended;

use super::record::{Stored, count_approval, hex_of, refusal, store_full, tail_head};
#[cfg(feature = "sealed")]
use super::sealed_impl::{self, SealedWrite};
use super::{
    Held, Index, MAX_RECORDS_PER_RUN, MAX_STORE_BYTES, RunIndex, StoreError, StoreLimitKind, record,
};

/// One staged append: the record to write and the step key it is for.
pub(super) struct Staged {
    /// The record as it will be archived.
    pub(super) record: Stored,
    /// The step key the already-recorded and conflict checks are on.
    pub(super) key: StepKey,
    /// Who wraps this record's data key and when it expires, when the store
    /// was opened sealed. `None` for every plain append.
    #[cfg(feature = "sealed")]
    pub(super) sealed: Option<SealedWrite>,
}

/// The ordered append, over the file and the index it folds into.
pub(super) fn append_on_owner(
    file: &mut File,
    shared: &Arc<Mutex<Index>>,
    staged: &Staged,
) -> std::io::Result<Stage<Appended, Arc<Mutex<Index>>>> {
    let mut index = owner::lock(shared);
    decide_and_write(file, &mut index, staged)
}

/// The decision, the write, and the fold this record owes its batch's flush.
fn decide_and_write(
    file: &mut File,
    index: &mut Index,
    staged: &Staged,
) -> std::io::Result<Stage<Appended, Arc<Mutex<Index>>>> {
    let stored = &staged.record;
    let key = staged.key;
    #[cfg(feature = "sealed")]
    let sealed = staged.sealed.as_ref();
    let owned = index.runs.get(&stored.run);
    if let Some(owned) = owned {
        if owned.tenant != stored.tenant {
            let refusal = Err(refusal(StoreError::ForeignTenant {
                owner: owned.tenant.clone(),
                asked: stored.tenant.clone(),
            }));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
            return refusal;
        }
        if let Some(step) = owned.steps.get(&key.to_hex()) {
            #[cfg(feature = "sealed")]
            if let Some(sealed) = step.sealed.as_ref() {
                let answer = sealed_impl::sealed_duplicate(sealed, &stored.value);
                return Ok(Stage::Settled(Ok(answer)));
            }
            return Ok(Stage::Settled(Ok(if step.bytes == stored.value {
                Appended::AlreadyRecorded
            } else {
                Appended::Conflicting
            })));
        }
        #[cfg(feature = "sealed")]
        if let Some(burned) = sealed_impl::refuse_if_burned(index, stored.run, &key.to_hex()) {
            let refusal = Err(refusal(burned));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: the step's slot is burned");
            return refusal;
        }
        if u64::saturating_from(owned.steps.len()) >= MAX_RECORDS_PER_RUN {
            let refusal = Err(refusal(StoreError::Limit {
                kind: StoreLimitKind::Records,
                requested: u64::saturating_from(owned.steps.len()).saturating_add(1),
                limit: MAX_RECORDS_PER_RUN,
            }));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
            return refusal;
        }
    }

    let staged_key = (stored.run, key.to_hex());
    if let Some(bytes) = index.staged.get(&staged_key) {
        return Ok(Stage::Settled(Ok(if *bytes == stored.value {
            Appended::AlreadyRecorded
        } else {
            Appended::Conflicting
        })));
    }

    let previous = tail_head(index);
    #[cfg(feature = "sealed")]
    let (framed, head, sealed_slot) =
        sealed_impl::frame_stored(stored, &previous, sealed).map_err(refusal)?;
    #[cfg(not(feature = "sealed"))]
    let (framed, head) = record::frame(stored, &previous).map_err(refusal)?;
    #[cfg(not(feature = "sealed"))]
    let _sealed_slot: Option<()> = None;

    let staged_bytes = u64::saturating_from(framed.len());
    let next = index
        .committed
        .checked_add(staged_bytes)
        .ok_or_else(|| refusal(store_full(u64::MAX)))?;
    if next > MAX_STORE_BYTES {
        let refusal = Err(refusal(store_full(next)));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
        return refusal;
    }

    let on_disk = file
        .metadata()
        .map_err(StoreError::storage)
        .map_err(refusal)?
        .len();
    if on_disk != index.committed {
        let held = match index.runs.get(&stored.run) {
            Some(run) => run.steps.len(),
            None => 0,
        };
        let refusal = Err(refusal(StoreError::Corrupt {
            at: u64::saturating_from(held),
        }));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
        return refusal;
    }

    index.committed = next;
    index.tail = head;
    index.staged.insert(staged_key, stored.value.clone());

    file.write_all(&framed)
        .map_err(StoreError::storage)
        .map_err(refusal)?;

    #[cfg(feature = "sealed")]
    let held = sealed_impl::held_for_settle(stored, sealed_slot);
    #[cfg(not(feature = "sealed"))]
    let held = Held {
        path: stored.path.clone(),
        bytes: stored.value.clone(),
    };
    let key = hex_of(&stored.key);
    let run = stored.run;
    let tenant = stored.tenant.clone();
    let definition = stored.definition.to_identity();
    #[cfg(feature = "sealed")]
    let count_bytes = stored.value.clone();
    Ok(Stage::Unsynced {
        answer: Appended::Recorded,
        bytes: framed.len(),
        settle: Box::new(move |index: &mut Arc<Mutex<Index>>| {
            #[cfg(feature = "sealed")]
            let mut held = held;
            let mut index = owner::lock(index);
            index.staged.remove(&(run, key.clone()));
            #[cfg(feature = "sealed")]
            count_approval(&mut index, &tenant, &count_bytes);
            #[cfg(not(feature = "sealed"))]
            count_approval(&mut index, &tenant, &held.bytes);
            #[cfg(feature = "sealed")]
            let ordinal = {
                let ordinal = index.frames;
                index.frames = index.frames.saturating_add(1);
                ordinal
            };
            let entry = index.runs.entry(run).or_insert_with(|| RunIndex {
                tenant: tenant.clone(),
                definition: definition.clone(),
                steps: HashMap::new(),
            });
            #[cfg(feature = "sealed")]
            if let Some(sealed) = held.sealed.as_mut() {
                sealed.ordinal = ordinal;
            }
            entry.steps.insert(key, held);
        }),
    })
}
