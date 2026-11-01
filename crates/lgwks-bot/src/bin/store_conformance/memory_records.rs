//! An in-memory [`RunRecords`]: a store of the caller's own, in the fewest lines
//! that keep every obligation the trait names.
//!
//! It is the conformance suite's test double and the shape a developer copies to
//! put their own database behind durable steps: `lookup`, `append` and
//! `record_count` over a map, a tenant check on every read and write, and the
//! definition each run was first written under. It survives nothing — not a
//! restart, not a crash — so it answers [`DurabilityPromise::Ephemeral`], and a
//! step that would hand an effect out of the process over it is refused.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lgwks_bot::effect::RunId;
use lgwks_bot::journal::DurabilityPromise;
use lgwks_bot::script::{Appended, FlowError, RunRecords, StepKey, StoredValue};
use lgwks_bot::task::{DefinitionIdentity, Drift, StoreError, StoreLimitKind};

/// The most records the double holds across every run, so a runaway test is a
/// typed refusal rather than an unbounded map.
pub const MAX_RECORDS: u64 = 65_536;

/// One run's records.
struct Run {
    /// The tenant that wrote the run's first record, and the only one that may
    /// read or extend it.
    tenant: String,
    /// The definition the run's first record was written under.
    definition: DefinitionIdentity,
    /// Per step key: the step's path and its archived value.
    steps: HashMap<[u8; 32], (String, Vec<u8>)>,
}

/// The double's whole state, shared by every handle.
#[derive(Default)]
struct State {
    /// Per run, its records.
    runs: HashMap<RunId, Run>,
    /// Records held across every run, against [`MAX_RECORDS`].
    held: u64,
}

/// An in-memory store; every clone is a handle onto the same records.
#[derive(Clone, Default)]
pub struct MemoryRecords {
    /// The records every handle shares.
    state: Arc<Mutex<State>>,
    /// Armed by [`MemoryRecords::fail_next_read`], taken by the next step read.
    unreadable: Arc<AtomicBool>,
}

impl MemoryRecords {
    /// Make the next step read — a lookup or a compatibility check — fail once.
    pub fn fail_next_read(&self) {
        self.unreadable.store(true, Ordering::SeqCst);
    }

    /// Spend an armed fault, refusing this read.
    fn readable(&self) -> Result<(), FlowError> {
        if self.unreadable.swap(false, Ordering::SeqCst) {
            let refusal = Err(FlowError::from(StoreError::Storage {
                cause: std::io::Error::other("the in-memory store refused a read on purpose"),
            }));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "readable: returning an error to the caller");
            return refusal;
        }
        Ok(())
    }

    /// The state, or the refusal a poisoned lock is.
    fn state(&self) -> Result<MutexGuard<'_, State>, FlowError> {
        self.state.lock().map_err(|poisoned| {
            FlowError::from(StoreError::Storage {
                cause: std::io::Error::other(format!("the in-memory store's lock: {poisoned}")),
            })
        })
    }
}

/// The refusal for `asked` reaching a run `owner` wrote.
fn foreign(owner: &str, asked: &str) -> FlowError {
    FlowError::from(StoreError::ForeignTenant {
        owner: owner.to_owned(),
        asked: asked.to_owned(),
    })
}

impl RunRecords for MemoryRecords {
    fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<StoredValue>, FlowError> {
        self.readable()?;
        let state = self.state()?;
        let Some(held) = state.runs.get(&run) else {
            return Ok(None);
        };
        if held.tenant != tenant {
            let refusal = Err(foreign(&held.tenant, tenant));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "lookup: returning an error to the caller");
            return refusal;
        }
        Ok(held
            .steps
            .get(key.as_bytes())
            .map(|step| StoredValue::new(step.0.clone(), step.1.clone())))
    }

    fn append(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
        path: &str,
        definition: &DefinitionIdentity,
        bytes: Vec<u8>,
    ) -> Result<Appended, FlowError> {
        let mut state = self.state()?;
        // Every check before any write, so a refusal changes nothing.
        if let Some(held) = state.runs.get(&run) {
            if held.tenant != tenant {
                let refusal = Err(foreign(&held.tenant, tenant));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "append: returning an error to the caller");
                return refusal;
            }
            if let Some(step) = held.steps.get(key.as_bytes()) {
                return Ok(if step.1 == bytes {
                    Appended::AlreadyRecorded
                } else {
                    Appended::Conflicting
                });
            }
        }
        if state.held >= MAX_RECORDS {
            let refusal = Err(FlowError::from(StoreError::Limit {
                kind: StoreLimitKind::Records,
                requested: state.held.saturating_add(1),
                limit: MAX_RECORDS,
            }));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "append: returning an error to the caller");
            return refusal;
        }
        state.held = state.held.saturating_add(1);
        state
            .runs
            .entry(run)
            .or_insert_with(|| Run {
                tenant: tenant.to_owned(),
                definition: definition.clone(),
                steps: HashMap::new(),
            })
            .steps
            .insert(*key.as_bytes(), (path.to_owned(), bytes));
        Ok(Appended::Recorded)
    }

    fn compatibility(
        &self,
        run: RunId,
        definition: &DefinitionIdentity,
    ) -> Result<bool, FlowError> {
        self.readable()?;
        let state = self.state()?;
        Ok(match state.runs.get(&run) {
            Some(held) => definition.drift_from(&held.definition).is_none(),
            None => true,
        })
    }

    fn drift(&self, run: RunId, definition: &DefinitionIdentity) -> Result<Drift, FlowError> {
        let state = self.state()?;
        let recorded = state
            .runs
            .get(&run)
            .map(|held| &held.definition)
            .ok_or_else(|| FlowError::failed("the in-memory store holds no record for this run"))?;
        definition
            .drift_from(recorded)
            .ok_or_else(|| FlowError::failed("the definitions agree, so there is no drift axis"))
    }

    /// Nothing here survives the process.
    fn durability(&self) -> DurabilityPromise {
        DurabilityPromise::Ephemeral
    }

    fn recorded_owner(&self, run: RunId) -> Result<Option<String>, FlowError> {
        Ok(self.state()?.runs.get(&run).map(|held| held.tenant.clone()))
    }

    fn recorded_definition(&self, run: RunId) -> Result<Option<DefinitionIdentity>, FlowError> {
        Ok(self
            .state()?
            .runs
            .get(&run)
            .map(|held| held.definition.clone()))
    }

    fn record_count(&self, run: RunId) -> Result<usize, FlowError> {
        Ok(match self.state()?.runs.get(&run) {
            Some(held) => held.steps.len(),
            None => 0,
        })
    }
}
