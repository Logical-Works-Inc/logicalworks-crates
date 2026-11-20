//! The ordered append and stage/settle logic for the durable run store.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::effect::RunId;
use crate::journal::frame::SaturatingFrom;
use crate::journal::owner::{self, Stage};
use crate::script::StepKey;
use crate::script::run_store::Appended;

use super::record::{Stored, count_approval, hex_of, refusal, store_full, tail_head};
#[cfg(feature = "sealed")]
use super::sealed_impl::{self, SealedWrite};
#[cfg(not(feature = "sealed"))]
use super::{Held, record};
use super::{Index, MAX_RECORDS_PER_RUN, MAX_STORE_BYTES, RunIndex, StoreError, StoreLimitKind};

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

/// The answer the store already has for `stored` before anything is written, or
/// `None` when the record is new and must be framed and appended.
///
/// A record for another tenant's run, a step whose slot is burned and a run at
/// its record ceiling are refused. A step already held, or already staged in
/// this batch, answers as recorded when the bytes match and as conflicting when
/// they do not.
fn prior_answer(index: &Index, stored: &Stored, key: StepKey) -> std::io::Result<Option<Appended>> {
    if let Some(owned) = index.runs.get(&stored.run) {
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
                return Ok(Some(sealed_impl::sealed_duplicate(sealed, &stored.value)));
            }
            return Ok(Some(echoed(&step.bytes, &stored.value)));
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
    match index.staged.get(&(stored.run, key.to_hex())) {
        Some(bytes) => Ok(Some(echoed(bytes, &stored.value))),
        None => Ok(None),
    }
}

/// The answer to a record whose step already holds `held`: recorded when
/// `offered` is the same bytes, conflicting when it is not.
fn echoed<T: PartialEq>(held: &T, offered: &T) -> Appended {
    if held == offered {
        Appended::AlreadyRecorded
    } else {
        Appended::Conflicting
    }
}

/// The store size once `framed_len` more bytes are on the file, checked before
/// they are written: refused past the store bound, and refused as corrupt when
/// the file is not the length the index committed.
fn room_for(file: &File, index: &Index, run: RunId, framed_len: usize) -> std::io::Result<u64> {
    let next = index
        .committed
        .checked_add(u64::saturating_from(framed_len))
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
        let held = match index.runs.get(&run) {
            Some(held_run) => held_run.steps.len(),
            None => 0,
        };
        let refusal = Err(refusal(StoreError::Corrupt {
            at: u64::saturating_from(held),
        }));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
        return refusal;
    }
    Ok(next)
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
    if let Some(answer) = prior_answer(index, stored, key)? {
        return Ok(Stage::Settled(Ok(answer)));
    }
    let staged_key = (stored.run, key.to_hex());

    let previous = tail_head(index);
    #[cfg(feature = "sealed")]
    let (framed, head, sealed_slot) =
        sealed_impl::frame_stored(stored, &previous, sealed).map_err(refusal)?;
    #[cfg(not(feature = "sealed"))]
    let (framed, head) = record::frame(stored, &previous).map_err(refusal)?;
    #[cfg(not(feature = "sealed"))]
    let _sealed_slot: Option<()> = None;

    let next = room_for(file, index, stored.run, framed.len())?;
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
