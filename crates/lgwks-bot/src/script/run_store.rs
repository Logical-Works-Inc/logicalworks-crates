//! The durable-step contract a [`Scope`] reaches without naming a host.
//!
//! # Why the store rides on the scope rather than a task-local
//!
//! A task-local does not cross a spawn, and
//! [`join_all_bounded`](crate::rt::task::join_all_bounded) spawns every input, so
//! a spawned step read no store and no definition and ran its body unrecorded
//! (#909). The host attaches the store and the definition to the run's root
//! scope, beside the authority; every descended scope shares them by clone, so a
//! step polled on a spawned task records against the same run and replays under
//! the same definition as the body. A scope no host admitted carries none, and a
//! local flow keeps claiming no durability.
//!
//! The trait is what [`remember`] calls, so the script module has no
//! dependency on [`lgwks_bot::task`](crate::task) and the task module supplies
//! the one shipped implementation. A caller that wants a store of their own
//! implements it, installs it with
//! [`HostBuilder::records`](crate::task::HostBuilder::records), and must honour
//! the same three obligations: [`RunRecords::lookup`] distinguishes a read
//! failure from absence, [`RunRecords::append`] returns only after the bytes are
//! durable, and both bind the caller's tenant. The `conformance` module (feature
//! `testing`) checks them.

use std::sync::Arc;

use crate::effect::RunId;
use crate::journal::DurabilityPromise;
pub use crate::journal::Durable;
use crate::task::{DefinitionIdentity, Drift, StoreError};

use super::{FlowError, Scope, StepKey};

#[cfg(feature = "testing")]
pub mod conformance;
mod step;

#[cfg(test)]
pub(crate) use step::definition_of;
pub use step::{remember, remembered_at};
pub(crate) use step::{remember_at, step_definition};

/// The authority a run's steps are checked against: the host's grant plus the
/// delta of any repair this run is carrying.
///
/// Object-safe and `Arc`-held: the host attaches it to the run's root
/// [`Scope`](crate::script::Scope) as a `dyn`, and every descended scope shares
/// it, so a step body can ask what it may do without the host being named in
/// its signature. On the scope rather than in a task-local because a task-local
/// does not cross a spawn, and a missing one read as "covered" (#909).
#[derive(Clone)]
pub(crate) struct Authority(pub(crate) Arc<dyn AuthorityCheck>);

/// What a step's `require` asks of the run's authority.
///
/// One method, because there is one question: *is this capability covered right
/// now*. It returns the whole shortfall rather than a `bool`, so a step that is
/// short of three capabilities learns all three at once and the repair ticket
/// built from the answer is written once.
pub(crate) trait AuthorityCheck: Send + Sync {
    /// Every capability in `required` this run's authority does not cover, in
    /// declaration order, each named once.
    fn uncovered(&self, required: &[crate::cap::Cap]) -> Vec<crate::cap::Shortage>;

    /// The registry the run's `observe` and `act` resolve against, when the
    /// host was given one.
    ///
    /// Asked of the authority rather than installed beside it because it is
    /// the same kind of fact — what the host lets this run name — and a fourth
    /// task-local would widen the frame every nested run carries (see the
    /// composition note on the host's run path).
    fn domains(&self) -> Option<&'static crate::registry::DomainRegistry>;
}

impl Authority {
    /// Every capability in `required` this authority does not cover.
    pub(crate) fn uncovered(&self, required: &[crate::cap::Cap]) -> Vec<crate::cap::Shortage> {
        self.0.uncovered(required)
    }

    /// The registry the host installed for this run, if any.
    pub(crate) fn domains(&self) -> Option<&'static crate::registry::DomainRegistry> {
        self.0.domains()
    }
}

/// The durable store a run's durable steps consult.
///
/// Object-safe, because it is installed as a `dyn` behind a task-local and
/// reached through `Arc<dyn RunRecords>`; it takes only borrowed arguments, so
/// it needs no generic method and no `Box`.
pub trait RunRecords: Send + Sync {
    /// The record committed for `key` under `run`, or `None` when there is
    /// none.
    ///
    /// `Err` is a read failure, never a miss: reporting one as `None` would make
    /// a step that had finished run again (INV-BOT-7).
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store cannot be read, including when `run` belongs
    /// to a tenant other than the one asking.
    fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<StoredValue>, FlowError>;

    /// Commit the record for `key` under `run`, durably, and return only once
    /// the bytes are on the device.
    ///
    /// # Errors
    ///
    /// [`FlowError`] for any refusal, including a declared ceiling. A refusal
    /// never truncates a committed record.
    fn append(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
        path: &str,
        definition: &DefinitionIdentity,
        bytes: Vec<u8>,
    ) -> Result<Appended, FlowError>;

    /// [`RunRecords::append`], waited for rather than sat through.
    ///
    /// The door [`remember`] takes, and the reason it is a separate method rather
    /// than an `async fn` on the trait: a durable write is `write_all` plus
    /// `sync_all`, and running that on the thread awaiting it parks every other
    /// task on that executor for the length of an `fsync`. A store that implements
    /// this hands the write to a thread of its own and this future waits for the
    /// answer, so the device's latency is the device's and the runtime keeps
    /// turning (INV-BOT-50).
    ///
    /// The default implementation calls [`RunRecords::append`] on the awaiting
    /// thread, which is correct and is exactly the defect this method exists to
    /// remove — so a store that has no owner thread is a store that blocks, not one
    /// that loses the record.
    ///
    /// # Errors
    ///
    /// Whatever [`RunRecords::append`] reports.
    fn append_async<'a>(
        &'a self,
        record: StagedRecord<'a>,
    ) -> crate::BoxFuture<'a, Result<Appended, FlowError>> {
        Box::pin(async move {
            let (tenant, run, key, path, definition) = (
                record.tenant(),
                record.run(),
                record.key(),
                record.path(),
                record.definition(),
            );
            self.append(tenant, run, key, path, definition, record.into_bytes())
        })
    }

    /// Whether this store's records for `run` were written under `definition`.
    ///
    /// The replay's own pre-flight, asked by every durable step before it returns
    /// a recorded value. A store that holds no record for the run at all answers
    /// `true`: that is a first attempt, and refusing it would make the first
    /// record unreachable.
    ///
    /// The default answers `true` because a store with no identity of its own has
    /// nothing to compare against — but a store that *does* have one must report a
    /// refusal as `Err` and never as `false`. A `false` is a claim that the
    /// records disagree with this definition, and a store that could not read its
    /// own records has established no such thing (INV-BOT-7): collapsing the two
    /// reports an unreadable store as a drifted one, which sends the caller
    /// looking for a change to a definition when the device is what failed.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store could not answer the question. The caller
    /// treats it as a refusal rather than as a compatibility, and it carries the
    /// store's own typed error rather than a rendering of it.
    fn compatibility(
        &self,
        _run: RunId,
        _definition: &DefinitionIdentity,
    ) -> Result<bool, FlowError> {
        Ok(true)
    }

    /// Which axis of `run`'s records disagrees with `definition`.
    ///
    /// A second door beside [`RunRecords::compatibility`] and not a return value
    /// of it, because the two answer different questions. `compatibility` answers
    /// *whether* to proceed and must be cheap enough to ask on every durable step;
    /// `drift` answers *which axis* and is asked only after a step has already been
    /// told no, where the caller needs a repair — a new run, a decision, a
    /// migration or an edit.
    ///
    /// Only the store knows the identity its records were written under, so the
    /// caller cannot recompute this from the declared identity alone; folding it
    /// into the boolean would have meant returning an `Option<Drift>` in place of
    /// an answer that is also legitimately `None`, and a caller that ignored the
    /// payload would have had no axis to report.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store holds no record for `run`, and therefore has
    /// no recorded identity to compare against. That is a state a caller cannot
    /// reach through [`RunRecords::compatibility`] — it answers `true` for a run
    /// the store never wrote — so the error is a refusal to invent an answer, not
    /// a path the shipped step takes. A store with no identity of its own does not
    /// need this door: it may report no drift, consistent with its `compatibility`
    /// answering `true`.
    fn drift(&self, _run: RunId, _definition: &DefinitionIdentity) -> Result<Drift, FlowError> {
        Err(FlowError::failed(
            "this store holds no definition identity for the run, so it cannot name a \
             drift axis; refusing to invent one",
        ))
    }

    /// Undecided approvals `tenant` holds (the `await approval` ceiling), `None` when this store keeps no count; a failed count is an error, never zero (INV-BOT-7).
    fn pending_count(&self, _tenant: &str) -> Result<Option<u64>, FlowError> {
        Ok(None)
    }

    /// Build the record this store is being asked to commit.
    ///
    /// One constructor for the six fields both doors take, because the awaited door
    /// and the blocking door are otherwise two places that assemble the same thing
    /// and could disagree about what a record is.
    fn stage<'a>(
        &self,
        tenant: &'a str,
        run: RunId,
        key: StepKey,
        path: &'a str,
        definition: &'a DefinitionIdentity,
        bytes: Vec<u8>,
    ) -> StagedRecord<'a> {
        StagedRecord::of(tenant, run, key, path, definition, bytes)
    }

    /// What this store promises about a record once an append has answered.
    ///
    /// The default is [`DurabilityPromise::Ephemeral`], the one answer a store that
    /// says nothing can be held to. A store that survives its writer dying says so,
    /// and only such a store may stand behind a step that hands an effect out of
    /// the process: [`act`](super::act) refuses the rest, because after a crash an
    /// ephemeral store reports an empty run rather than an uncertain one.
    fn durability(&self) -> DurabilityPromise {
        DurabilityPromise::Ephemeral
    }

    /// The tenant that owns `run`, when this store holds records for it.
    ///
    /// The host asks before it adopts a run id, so a resume of another tenant's
    /// run is refused before admission rather than at its first step. `Ok(None)`
    /// is a run this store never wrote, which is not a refusal: a process killed
    /// before its first record left nothing to own. The default answers `None`,
    /// and [`RunRecords::lookup`] still refuses the foreign read on its own.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store cannot answer, never reported as `None`
    /// (INV-BOT-7).
    fn recorded_owner(&self, _run: RunId) -> Result<Option<String>, FlowError> {
        Ok(None)
    }

    /// The definition `run`'s records were written under, when this store keeps
    /// one.
    ///
    /// What a resume writes its records under, so a resume cannot check its
    /// definition against one run and record under another. `Ok(None)` for a run
    /// with no records, and for a store that keeps no identity of its own, which
    /// then records under the identity the caller declared.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store cannot answer.
    fn recorded_definition(&self, _run: RunId) -> Result<Option<DefinitionIdentity>, FlowError> {
        Ok(None)
    }

    /// How many records this store holds for `run`, `0` for a run it never wrote.
    ///
    /// What a report's step-record count and an in-flight request's count carry.
    /// Required rather than defaulted, because a default could only answer zero,
    /// and zero is a claim that nothing was recorded.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store cannot answer; the report then names the
    /// run's records as uncounted rather than as none (INV-BOT-7).
    fn record_count(&self, run: RunId) -> Result<usize, FlowError>;
}

/// One record a store is being asked to commit, borrowed rather than assembled at
/// each call site.
///
/// The trait's two doors take the same six facts, and a store's implementation
/// wants them as one value; a struct the caller fills once is what keeps those
/// two doors from being two copies of the
/// same parameter list.
///
/// Fields private and reached through accessors, because the invariants a store
/// checks — the tenant that owns the run, the key the record is filed under, the
/// definition it was written under — are the store's to make, not a caller's to
/// have bypassed by writing the field directly.
#[derive(Debug, Clone)]
pub struct StagedRecord<'a> {
    /// The tenant the run belongs to.
    tenant: &'a str,
    /// The run the record is for.
    run: RunId,
    /// The step key it is filed under.
    key: StepKey,
    /// The step's path, for attribution.
    path: &'a str,
    /// The definition the value is recorded under.
    definition: &'a DefinitionIdentity,
    /// The archived value.
    bytes: Vec<u8>,
}

impl<'a> StagedRecord<'a> {
    /// The six facts a record is, assembled once.
    fn of(
        tenant: &'a str,
        run: RunId,
        key: StepKey,
        path: &'a str,
        definition: &'a DefinitionIdentity,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            tenant,
            run,
            key,
            path,
            definition,
            bytes,
        }
    }

    /// The tenant the run belongs to.
    #[must_use]
    pub const fn tenant(&self) -> &'a str {
        self.tenant
    }

    /// The run this stored record was filed under, which names the run that wrote it.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.run
    }

    /// The step key the record is filed under.
    #[must_use]
    pub const fn key(&self) -> StepKey {
        self.key
    }

    /// The step's path, for attribution.
    #[must_use]
    pub const fn path(&self) -> &'a str {
        self.path
    }

    /// The definition the value is recorded under.
    ///
    /// Borrowed rather than cloned because the store copies it once, onto its own
    /// storage thread, where the frame head is computed — the same place the
    /// tenant and path are copied, so a store cannot record one without the other.
    #[must_use]
    pub const fn definition(&self) -> &'a DefinitionIdentity {
        self.definition
    }

    /// The archived value, by reference.
    ///
    /// A borrow rather than a `take`, so reading a staged record cannot empty it
    /// behind a store that is about to write it.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The archived value, by value, for a store that moves it onto its own
    /// storage thread or into its own table.
    #[must_use]
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A committed record, as the script module receives it: the step's path and its
/// archived bytes.
///
/// The bytes rather than the decoded value because decoding needs the caller's
/// type `T`, which the store does not know. Holding the path beside the value is
/// what lets a decode failure be attributed to the step rather than to the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredValue {
    /// The step path this record is about.
    path: String,
    /// The archived value.
    bytes: Vec<u8>,
}

impl StoredValue {
    /// Build a record from the path and archived bytes the store holds.
    ///
    /// The one door a store of the caller's own answers [`RunRecords::lookup`]
    /// through. The fields stay private, so a value is read back only by
    /// [`super::remember`], which decodes it under the step's own type.
    #[must_use]
    pub fn new(path: String, bytes: Vec<u8>) -> Self {
        Self { path, bytes }
    }

    /// The archived bytes this record holds, for a host that stores a fixed-width
    /// reserved value (a request digest, a terminal record) under a step key.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The archived bytes, by value.
    ///
    /// The reader's door, and the counterpart to the borrow
    /// [`StagedRecord::bytes`] takes on the writer's side. `pub(crate)` for the same
    /// reason [`StoredValue::new`] is: the store hands these out and
    /// [`crate::task::RunStore::lookup`] is the one place a caller may read them,
    /// so no public field can hand a step of the wrong type a value it will decode.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// What a durable append did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Appended {
    /// The record is committed and durable.
    Recorded,
    /// An identical record was already committed; nothing new was written and
    /// the step is known durable.
    AlreadyRecorded,
    /// A different record is committed under this key. Nothing was written; the
    /// caller must not treat the step as re-recorded.
    Conflicting,
}

/// The erased store handle a run installs.
#[derive(Clone)]
pub(crate) struct Records(pub(crate) Arc<dyn RunRecords>);

impl Records {
    /// The typed lookup this module performs, or `None` when no store is
    /// installed.
    pub(crate) fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<StoredValue>, FlowError> {
        self.0.lookup(tenant, run, key)
    }

    /// Whether this store's records for `run` agree with `definition`, or the
    /// store's own refusal.
    ///
    /// The check every durable step makes before it replays a value, and the
    /// reason a store that was handed a run from elsewhere cannot answer from
    /// another definition's records. `Ok(())` when the store holds no record for
    /// the run at all, which is the same answer it gives a first attempt.
    ///
    /// Both failures leave as `Err`, and they are *different* errors. A store
    /// refusal is propagated as itself, because it says nothing about the
    /// definition: folding it into a boolean would tell the caller "recorded under
    /// a different definition" — a specific, actionable claim — about a store
    /// whose answer nobody knows, and would make the one fault an operator must
    /// see indistinguishable from a genuine drift (INV-BOT-7). A disagreement
    /// leaves as the typed [`FlowError::Incompatible`] carrying the axis, so the
    /// caller learns whether it needs a new run, a decision, a migration or an
    /// edit rather than having to parse a rendered sentence to find out.
    ///
    /// The two are raised from one call rather than from a boolean plus a second
    /// question, because a store that answers "no" and then cannot name the axis
    /// would otherwise leave the caller with a refusal it cannot repair — and
    /// inventing an axis at that point is exactly the guess the typed arm rules
    /// out.
    ///
    /// # Errors
    ///
    /// [`FlowError::Incompatible`] carrying the drift, or whatever typed error the
    /// store itself reported.
    pub(crate) fn agrees(
        &self,
        run: RunId,
        definition: &DefinitionIdentity,
    ) -> Result<(), FlowError> {
        match self.0.compatibility(run, definition) {
            Ok(true) => Ok(()),
            Ok(false) => Err(FlowError::incompatible("", self.0.drift(run, definition)?)),
            Err(error) => Err(error),
        }
    }

    /// The durable append, refusing a conflicting record as an error.
    ///
    /// A conflict is turned into a [`FlowError`] here rather than being left as
    /// an [`Appended`] arm the caller must check, because the one correct
    /// response to a conflicting record is to refuse the step, and a caller who
    /// forgets to check the arm would silently keep a rewritten history.
    ///
    /// # Errors
    ///
    /// [`FlowError`] on a refusal or on a conflict.
    pub(crate) fn record<'a>(
        &'a self,
        tenant: &'a str,
        run: RunId,
        key: StepKey,
        path: &'a str,
        definition: &'a DefinitionIdentity,
        bytes: Vec<u8>,
    ) -> crate::BoxFuture<'a, Result<(), FlowError>> {
        let conflict = std::sync::Arc::from(path);
        Box::pin(async move {
            match self
                .0
                .append_async(self.0.stage(tenant, run, key, path, definition, bytes))
                .await
            {
                Ok(Appended::Recorded) | Ok(Appended::AlreadyRecorded) => Ok(()),
                Ok(Appended::Conflicting) => Err(FlowError::Failed {
                    at: conflict,
                    reason: format!(
                        "a different value is already recorded for step {path:?} under this run; \
                         refusing to overwrite a committed record"
                    ),
                }),
                Err(error) => Err(error.located_at(&conflict)),
            }
        })
    }

    /// The durable append, reporting which [`Appended`] arm it landed on.
    ///
    /// The door a request key's first submission takes: `Recorded` is the caller
    /// that may run the body, `AlreadyRecorded` is one that must reattach, and
    /// `Conflicting` is a different payload under the same key. [`Self::record`]
    /// collapses the first two because a durable *step* treats them the same;
    /// a submission cannot, so it needs the arm itself.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the store refuses the read or the write.
    pub(crate) async fn claim<'a>(
        &'a self,
        tenant: &'a str,
        run: RunId,
        key: StepKey,
        path: &'a str,
        definition: &'a DefinitionIdentity,
        bytes: Vec<u8>,
    ) -> Result<Appended, FlowError> {
        self.0
            .append_async(self.0.stage(tenant, run, key, path, definition, bytes))
            .await
    }

    /// The tenant that owns `run`, as the store recorded it.
    ///
    /// # Errors
    ///
    /// Whatever the store reports when it cannot answer.
    pub(crate) fn owner(&self, run: RunId) -> Result<Option<String>, FlowError> {
        self.0.recorded_owner(run)
    }

    /// The identity `run`'s records were written under, when the store keeps one.
    ///
    /// # Errors
    ///
    /// Whatever the store reports when it cannot answer.
    pub(crate) fn recorded_definition(
        &self,
        run: RunId,
    ) -> Result<Option<DefinitionIdentity>, FlowError> {
        self.0.recorded_definition(run)
    }

    /// How many records the store holds for `run`.
    ///
    /// # Errors
    ///
    /// Whatever the store reports when it cannot answer.
    pub(crate) fn record_count(&self, run: RunId) -> Result<usize, FlowError> {
        self.0.record_count(run)
    }

    /// What a report says about `run`'s step records.
    ///
    /// A count the store could not give is uncounted, never zero: a read failure
    /// is not absence (INV-BOT-7).
    pub(crate) fn knowledge(&self, run: RunId) -> crate::task::EffectKnowledge {
        match self.0.record_count(run) {
            Ok(records) => crate::task::EffectKnowledge::StepRecords { records },
            Err(unread) => {
                lgwks_std::trace::warn!(error = %unread, "knowledge: the store could not count the run's records");
                crate::task::EffectKnowledge::StepRecordsUncounted
            }
        }
    }

    /// Refuse an effect leaving the process when this store does not survive its
    /// writer dying.
    ///
    /// The step store's half of the journal's own rule: below
    /// [`DurabilityPromise::ProcessCrash`] a recovered run cannot tell "the effect
    /// never left" from "the record of it leaving was lost", and that is the state
    /// in which a blind resend happens.
    ///
    /// # Errors
    ///
    /// [`FlowError::Store`] carrying [`StoreError::PromiseUnmet`] for an
    /// ephemeral store.
    pub(crate) fn admit_external_handoff(&self) -> Result<(), FlowError> {
        let offered = self.0.durability();
        if offered.survives_process_crash() {
            return Ok(());
        }
        let refusal = Err(FlowError::from(StoreError::PromiseUnmet {
            required: DurabilityPromise::ProcessCrash,
            offered,
        }));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "admit_external_handoff: returning an error to the caller");
        refusal
    }
}

impl std::fmt::Debug for Records {
    /// The installed store's own `Debug`, so a host's debug output names the
    /// store it installed without the script module importing its type.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RunRecords")
    }
}

/// The definition `scope`'s run records under, when the host that admitted it
/// attached one.
///
/// `None` for a step outside a run, which is what a hand-built [`Scope`] with no
/// host above it is. The caller then derives its own, so the two doors — a
/// host-attached identity and a bare scope — cannot disagree about whether a run
/// declared one.
pub(crate) fn installed_definition(scope: &Scope) -> Option<Arc<DefinitionIdentity>> {
    scope.definition().cloned()
}

/// The store `scope`'s run records through, when the host that admitted it
/// attached one.
pub(crate) fn installed(scope: &Scope) -> Option<Records> {
    scope.records().cloned()
}
