//! The file store as the script module reaches it, through [`RunRecords`].

use crate::effect::RunId;
use crate::journal::DurabilityPromise;
use crate::journal::owner::SubmitError;
use crate::script::run_store::{Appended, RunRecords, StagedRecord, StoredValue};
use crate::script::{FlowError, StepKey};

use super::{DefinitionIdentity, Drift, RunStore, StoreError};

/// The store as the script module reaches it: a typed lookup and a typed append
/// over archived bytes, with no `lgwks_bot::task` in its own surface.
///
/// The implementation is here; the trait lives in
/// [`script`](mod@crate::script) because the task module is a *consumer* of a
/// durable step, and a step must be markable without the host that runs it.
impl RunRecords for RunStore {
    fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<StoredValue>, FlowError> {
        self.step_readable()?;
        let index = self.index();
        let Some(held) = index.runs.get(&run) else {
            return Ok(None);
        };
        if held.tenant != tenant {
            let refusal = Err(StoreError::ForeignTenant {
                owner: held.tenant.clone(),
                asked: tenant.to_owned(),
            }
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "lookup: returning an error to the caller");
            return refusal;
        }
        Ok(held
            .steps
            .get(&key.to_hex())
            .map(|step| StoredValue::new(step.path.clone(), step.bytes.clone())))
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
        let staged = self.owned(RunRecords::stage(
            self, tenant, run, key, path, definition, bytes,
        ));
        self.commit(staged.record, staged.key)
            .map_err(FlowError::from)
    }

    /// Whether the records for `run` were written under `definition`.
    ///
    /// The check every durable step makes before it returns a recorded value. A
    /// run this store holds nothing for is compatible by definition: that is a
    /// first attempt, and refusing it would make the first record unreachable.
    ///
    /// # Errors
    ///
    /// Whatever [`Self::check_definition`] reports, unchanged: a drift as
    /// [`FlowError::Incompatible`], and a device refusal as itself. Neither is ever
    /// collapsed into the other's answer.
    fn compatibility(
        &self,
        run: RunId,
        definition: &DefinitionIdentity,
    ) -> Result<bool, FlowError> {
        self.step_readable()?;
        self.check_definition(run, definition)
            .map(|()| true)
            .map_err(FlowError::from)
    }

    /// Which axis of `run`'s recorded identity disagrees with `definition`.
    ///
    /// The same comparison [`Self::compatibility`] made, returning the axis rather
    /// than the boolean. Two doors rather than one returning a richer type because
    /// the boolean is asked on every durable step — including the ones that pass,
    /// where the axis is never wanted — and building a [`Drift`] that names two
    /// digests or two schema ids on a step that will replay cleanly is work spent
    /// only to be thrown away.
    ///
    /// # Errors
    ///
    /// [`FlowError::Failed`] when this store holds no record for `run`, so there is
    /// no recorded identity to compare against, or when the two doors disagree —
    /// `compatibility` refused this pair while this comparison finds nothing to
    /// refuse. Naming an axis in either case would mean inventing one.
    fn drift(&self, run: RunId, definition: &DefinitionIdentity) -> Result<Drift, FlowError> {
        let Some(recorded) = self.definition_of(run) else {
            let refusal = Err(FlowError::failed(format!(
                "this store holds no definition identity for run {}, so it cannot name a drift \
             axis; refusing to invent one",
                run.id().to_hex()
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "drift: returning an error to the caller");
            return refusal;
        };
        match definition.drift_from(&recorded) {
            Some(drift) => Ok(drift),
            None => Err(FlowError::failed(format!(
                "run {} was refused as incompatible and yet agrees with the declared \
                 definition; refusing to name an axis for a disagreement that is not there",
                run.id().to_hex()
            ))),
        }
    }

    /// The awaited door, which is the one a durable step takes.
    ///
    /// The `sync_all` behind it is the device's cost, and this is what keeps that
    /// cost off the executor: the step's future waits for the owner's answer
    /// instead of sitting through the flush (INV-BOT-50).
    fn append_async<'a>(
        &'a self,
        record: StagedRecord<'a>,
    ) -> crate::BoxFuture<'a, Result<Appended, FlowError>> {
        let staged = self.owned(record);
        Box::pin(async move {
            self.commit_async(staged.record, staged.key)
                .await
                .map_err(FlowError::from)
        })
    }

    /// A record this store acknowledged was written and `sync_all`ed on the
    /// storage owner's thread before the answer was sent, so it survives the
    /// writer dying and, to the extent the device honours its flush, power loss.
    fn durability(&self) -> DurabilityPromise {
        DurabilityPromise::PowerLoss
    }

    /// The tenant the run's records name, read from the replayed index.
    fn recorded_owner(&self, run: RunId) -> Result<Option<String>, FlowError> {
        Ok(self.tenant_of(run))
    }

    /// How many parked approvals `tenant` holds undecided.
    ///
    /// The ceiling an `await approval` is admitted against, folded at replay
    /// and at every committed append, so answering it costs a probe however
    /// large the store is.
    fn pending_count(&self, tenant: &str) -> Result<Option<u64>, FlowError> {
        Ok(Some(self.pending_for(tenant)))
    }

    /// The identity the run's first record was written under.
    fn recorded_definition(&self, run: RunId) -> Result<Option<DefinitionIdentity>, FlowError> {
        Ok(self.definition_of(run))
    }

    /// The run's records in the replayed index.
    fn record_count(&self, run: RunId) -> Result<usize, FlowError> {
        Ok(RunStore::record_count(self, run))
    }
}

/// Map one owner refusal onto this store's vocabulary.
///
/// One conversion rather than a match per call site, because every `?` on an owner
/// refusal wraps the same fact — the device, or the handle, said no — and a literal
/// per call site would be four chances to name a different variant for one event.
/// The refusal's own message distinguishes a full queue from a poison from an
/// undelivered answer, so nothing is lost by carrying it as the device's error.
impl From<SubmitError> for StoreError {
    fn from(cause: SubmitError) -> Self {
        Self::Storage {
            cause: cause.into_io(),
        }
    }
}
