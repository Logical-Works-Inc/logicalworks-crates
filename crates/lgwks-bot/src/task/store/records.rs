//! The file store as the script module reaches it, through [`RunRecords`].

use crate::effect::RunId;
use crate::journal::DurabilityPromise;
use crate::journal::owner::SubmitError;
use crate::script::run_store::{Appended, RunRecords, StagedRecord, StoredValue};
use crate::script::{FlowError, StepKey};

use super::{DefinitionIdentity, Drift, RunStore, StoreError};

/// What a sealed open carries into `RunStore::open_impl`. Without `sealed`
/// there is no sealed open, so the option it rides in is always `None`.
#[cfg(feature = "sealed")]
pub(super) use super::sealed_impl::SealedStoreOpen;
#[cfg(not(feature = "sealed"))]
pub(super) type SealedStoreOpen = core::convert::Infallible;

/// Refuse a foreign tenant's read, logging the refusal where it is made.
///
/// One definition because the lookup refuses a foreign tenant in two places —
/// the burn set and the unknown run — and two spellings would be two chances
/// to name a different owner for one event.
fn foreign_tenant<T>(owner: String, asked: String) -> Result<T, FlowError> {
    lgwks_std::trace::debug!(
        owner = owner.as_str(),
        asked = asked.as_str(),
        "lookup: a foreign tenant reads another tenant's record"
    );
    Err(StoreError::ForeignTenant { owner, asked }.into())
}

/// Refuse a shredded step's read as erased, logging the refusal with it.
///
/// One definition because the live index and the unknown-step arm refuse a
/// tombstone with the same answer, and two spellings would drift.
#[cfg(feature = "sealed")]
fn sealed_erased<T>(run: RunId, key_hex: String) -> Result<T, FlowError> {
    lgwks_std::trace::debug!(
        run = run.id().to_hex(),
        key = key_hex.as_str(),
        "lookup: a shredded step reads as erased"
    );
    Err(StoreError::SealedErased {
        run: run.id().to_hex(),
        key: key_hex,
    }
    .into())
}

/// Refuse an unknown sealed step, logging the refusal with it.
///
/// One definition because the missing run and the missing step refuse with
/// the same typed answer, and two spellings would drift.
#[cfg(feature = "sealed")]
fn sealed_unknown<T>(run: RunId, key_hex: String) -> Result<T, FlowError> {
    lgwks_std::trace::debug!(
        run = run.id().to_hex(),
        key = key_hex.as_str(),
        "lookup: no sealed step names this key"
    );
    Err(StoreError::SealedUnknown {
        run: run.id().to_hex(),
        key: key_hex,
    }
    .into())
}

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
        let key_hex = key.to_hex();
        #[cfg(feature = "sealed")]
        {
            let index = self.index();
            if let Some(erased) = index.erased.get(&(run, key_hex.clone())) {
                if erased.tenant != tenant {
                    return foreign_tenant(erased.tenant.clone(), tenant.to_owned());
                }
                return sealed_erased(run, key_hex);
            }
        }
        let index = self.index();
        let Some(held) = index.runs.get(&run) else {
            #[cfg(feature = "sealed")]
            if self.inner.sealed.is_some() {
                if let Some(erased) = index.erased.get(&(run, key_hex.clone()))
                    && erased.tenant != tenant
                {
                    return foreign_tenant(erased.tenant.clone(), tenant.to_owned());
                }
                return sealed_unknown(run, key_hex);
            }
            return Ok(None);
        };
        if held.tenant != tenant {
            return foreign_tenant(held.tenant.clone(), tenant.to_owned());
        }
        let Some(step) = held.steps.get(&key_hex) else {
            #[cfg(feature = "sealed")]
            if self.inner.sealed.is_some() {
                return sealed_unknown(run, key_hex);
            }
            return Ok(None);
        };
        #[cfg(feature = "sealed")]
        if let Some(sealed) = step.sealed.as_ref() {
            use crate::sealed::fold::{Unwrapped, fold_unwrap};
            let provider = match self.inner.sealed.as_ref() {
                Some(provider) => std::sync::Arc::clone(provider),
                None => {
                    lgwks_std::trace::debug!(
                        ordinal = sealed.ordinal,
                        "lookup: no provider opens a sealed step"
                    );
                    return Err(StoreError::Corrupt { at: sealed.ordinal }.into());
                }
            };
            match fold_unwrap(provider.unwrap_key(&sealed.key_id, &sealed.wrapped)) {
                Ok(Unwrapped::Live(data_key)) => {
                    use crate::sealed::envelope::open as envelope_open;
                    let plaintext = match envelope_open(
                        &data_key,
                        &sealed.nonce,
                        &sealed.ciphertext,
                    ) {
                        Ok(plaintext) => plaintext,
                        Err(error) => {
                            lgwks_std::trace::debug!(error = ?error, "lookup: a live key with an unopenable envelope");
                            return Err(StoreError::Corrupt { at: sealed.ordinal }.into());
                        }
                    };
                    use lgwks_std::wire::{WireError, from_bytes};
                    let stored: super::record::Stored = match from_bytes::<
                        super::record::Stored,
                        WireError,
                    >(&plaintext)
                    {
                        Ok(stored) => stored,
                        Err(error) => {
                            lgwks_std::trace::debug!(error = ?error, "lookup: a committed frame does not decode");
                            return Err(StoreError::Corrupt { at: sealed.ordinal }.into());
                        }
                    };
                    return Ok(Some(StoredValue::new(stored.path, stored.value)));
                }
                Ok(Unwrapped::Erased) => {
                    return sealed_erased(run, key_hex);
                }
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "lookup: the provider refused the sealed step");
                    return Err(StoreError::SealedKey { source: error }.into());
                }
            }
        }
        Ok(Some(StoredValue::new(
            step.path.clone(),
            step.bytes.clone(),
        )))
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
        self.commit(staged).map_err(FlowError::from)
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
        Box::pin(async move { self.commit_async(staged).await.map_err(FlowError::from) })
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

impl RunStore {
    /// Up to `limit` raw approval records of `tenant`: run, key hex, path,
    /// bytes.
    ///
    /// The scan behind the pending listing: only records that decode as a
    /// parked approval are cloned, so a store of large step values is not
    /// copied to list a few approvals. Sorted by key, so two hosts list the
    /// same approvals in the same order.
    pub(crate) fn pending_scan(
        &self,
        tenant: &str,
        limit: usize,
    ) -> Vec<(RunId, String, String, Vec<u8>)> {
        use crate::script::approval::{RecordClass, classify_record};
        #[cfg(feature = "sealed")]
        let provider = self.inner.sealed.as_ref().map(std::sync::Arc::clone);
        let index = self.index();
        let mut out = Vec::new();
        let mut runs: Vec<RunId> = index
            .runs
            .iter()
            .filter_map(|pair| {
                if pair.1.tenant == tenant {
                    Some(*pair.0)
                } else {
                    None
                }
            })
            .collect();
        runs.sort_by_key(|item| item.id().to_hex());
        for run in runs {
            let Some(kept) = index.runs.get(&run) else {
                continue;
            };
            let mut keys: Vec<&String> = kept.steps.keys().collect();
            keys.sort();
            for key_hex in keys {
                if out.len() >= limit {
                    break;
                }
                let Some(step) = kept.steps.get(key_hex) else {
                    continue;
                };
                #[cfg(feature = "sealed")]
                if let Some(sealed) = step.sealed.as_ref() {
                    let Some(provider) = provider.as_ref() else {
                        continue;
                    };
                    let Some(value) = super::sealed_impl::pending_sealed_value(provider, sealed)
                    else {
                        continue;
                    };
                    if matches!(classify_record(&value), RecordClass::Pending(_)) {
                        out.push((run, key_hex.clone(), step.path.clone(), value));
                    }
                    continue;
                }
                if matches!(classify_record(&step.bytes), RecordClass::Pending(_)) {
                    out.push((run, key_hex.clone(), step.path.clone(), step.bytes.clone()));
                }
            }
            if out.len() >= limit {
                break;
            }
        }
        out.sort_by(|left, right| left.1.cmp(&right.1).then(left.2.cmp(&right.2)));
        out
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
