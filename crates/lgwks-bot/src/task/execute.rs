//! The one body of a run, shared by every door, and the reports a run the
//! host declined is given.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::cap::{Cap, Deficit, Demand, uncovered};
use crate::ecs::{RunFeed, RunFeedError, RunOutcome, RunTicket};
use crate::effect::RunId;
use crate::gate::GrantSet;
use crate::script::run_store::{Authority as RecordsAuthority, Records};
use crate::script::trail::Trail;
use crate::script::{FlowError, Scope, within};

use super::admit::{ClaimRefused, Declined, RunAuthority, Terminal, TrailSnapshot, disposition_of};
use super::host::Plan;
use super::name::TaskName;
use super::{
    BODY_STEP, Control, DefinitionIdentity, Disposition, HELD_PERMITS, Host, RepairTicket, Report,
    StoreError, Task,
};

impl Host {
    /// The one body of a run, shared by [`Host::run`], [`Host::resume`] and
    /// [`Host::repair`].
    ///
    /// Split so there is exactly one place a run mints or adopts its identity,
    /// checks its authority, builds its scope, installs the store and assembles
    /// its report — a second copy of any of those is a second definition of what
    /// a resume means. `repair` is the one door that widens authority, and it is a
    /// parameter rather than a branch, so the admission check below cannot be
    /// bypassed by a caller that happens to hold a ticket.
    ///
    /// The root budget is charged for **every** run, repair or not, and it is
    /// charged *before* the body so a run that spends its budget to zero reaches
    /// the refusal rather than running once more. That is what makes a permanent
    /// refusal plus repeated `NotApplied` finite: the budget is carried in the
    /// run's own ledger, so a fresh process finds it where the last one left it.
    ///
    /// `plan` carries the two facts a caller may declare — the run id to resume
    /// and the definition identity it asserts — as one sum type, so the drift
    /// check has one placement rather than one per door.
    pub(super) async fn execute<I, O, F, Fut>(
        &self,
        task: &Task<F>,
        input: I,
        plan: Plan,
        repair: Option<&RepairTicket>,
        grant_delta: &GrantSet,
        spend: u64,
    ) -> Report<O>
    where
        F: Fn(Scope, I) -> Fut,
        Fut: Future<Output = Result<O, FlowError>>,
    {
        let started = Instant::now();
        let task_name = task.name_owned();
        let resume = plan.run();
        // The definition this run records under. A run that declared none gets a
        // per-run identity derived from the tenant, the task and the run id, which
        // is stable across attempts at one run and therefore resumes exactly as a
        // run under `run` already does — a declared identity is an addition, never
        // a precondition.
        // A resume's identity names the run it is *replacing* only for its first
        // attempt: a caller that runs the same task on the same input twice gets
        // two runs and two identities, which is right — they are two pieces of
        // work. Declaring one identity for both and resuming the first would then
        // be refused, which is the opposite of what a caller means. So the
        // identity a run is recorded under is its own first attempt's, and a
        // resume is measured against that.
        // Read before the match consumes it: the drift check below needs to know
        // whether the caller declared this identity or the host derived it, and
        // only the first is a claim about the run.
        let declared = plan.declared().cloned();
        let definition = match declared.clone() {
            Some(identity) => identity,
            None => self.definition_for(task_name.as_str(), resume.as_ref()),
        };

        // A resume this store knows belongs to another tenant is refused before
        // admission, so the refusal is a `Refused` with no body and no step rather
        // than a record written under the wrong tenant. A run the store has
        // *never seen* is not refused: a process killed before its first record
        // left no evidence at all, and refusing that resume would make the
        // exactly-at-least-once step unrecoverable. The store still refuses to
        // *read* another tenant's records; this only governs whether the run id
        // may be adopted.
        if let Some(run) = resume
            && let Some(reason) = self.foreign_resume(run)
        {
            return self.refuse(started, task_name, reason);
        }
        // One resumer per run on this host (#1055). Reading the owner above does
        // not take the run: every concurrent resume of one decided run found its
        // steps unrecorded, ran their bodies and only then recorded them. The
        // claim is taken before admission, so a loser holds no permit and
        // writes nothing, and it is held to the end of this call, so a future the
        // caller drops releases it. A fresh run mints its own id and has no
        // rival, so it takes none.
        let _claim = match resume {
            Some(adopted) => match self
                .inner
                .in_flight
                .claim(self.inner.tenant.as_str(), adopted)
            {
                Ok(claim) => Some(claim),
                Err(refusal) => return self.claim_refused(started, task_name, adopted, refusal),
            },
            None => None,
        };
        // What this call's records will be *written* under, decided before
        // admission. The run a call adopts is not always the run it records
        // against — a fresh run mints one of its own below, and it is *that*
        // run's records this call writes — so a resume reads the identity its own
        // records already carry rather than the one its derivation would name.
        // The steps need that: a durable step must be able to *find* the records
        // it wrote under the identity the run was admitted with, whichever
        // declaration the caller makes now.
        let recorded_definition = match resume {
            Some(adopted) => self.identity_for(adopted, &definition),
            None => definition.clone(),
        };
        // The drift check, before admission and before the body is ever
        // constructed. Replaying a value the current definition never produced and
        // re-running a step whose effect already landed are the two answers on
        // either side of this, so the run is refused rather than picked between,
        // and the refusal names the axis that disagrees so the caller knows which
        // repair it needs. Nothing is written and no permit is taken.
        //
        // Only a *declared* identity is checked. The recorded one is what the
        // steps below write and look up under, so comparing a run's records
        // against it would agree by construction and fence nothing.
        if let Some(located) = self.drifted(resume, declared.as_ref(), &task_name) {
            self.inner.refused.fetch_add(1, Ordering::Relaxed);
            return self.unrun(started, task_name, Disposition::Refused, located, resume);
        }
        // The run identity, decided before admission so a refused run still has
        // the identity a caller can ask about — a run refused at admission ran
        // nothing, and a run id attached to it names a run with no records.
        let run = self.adopt_run(resume);
        if let Some(reason) = self.storeless(resume, run) {
            return self.refuse(started, task_name, reason);
        }

        // The authority check, at the admission boundary and as one complete pass.
        // Only the task's own declaration is checked here, and it is the blunt
        // form: a task that reaches in its *first* step declares it with
        // [`Task::requiring`] and is refused before its body, so nothing runs and
        // nothing is recorded. A task that reaches later declares nothing here and
        // asks at the step that reaches, through [`Scope::require`], so the work
        // before the block survives to be replayed by a repair.
        //
        // The base grant and this run's repair delta are checked *together* and
        // never one replacing the other: a repair adds authority for one run, and
        // every capability the host already had still holds.
        if let Some(deficit) = self.shortfall(task.needs(), &task_name, grant_delta) {
            // Blocked before admission and before the body: nothing was polled and
            // nothing executed, and the report names *every* unmet need plus the
            // repair ticket that closes exactly those. Both come from this one
            // `deficit`, so they cannot disagree about what the run was missing.
            return self.blocked(started, task_name, run, deficit);
        }

        // Admission first, so a run that never starts is never counted as one
        // and never leaves a step in the trail.
        let permit = match self.admit(started, &task_name).await {
            Ok(permit) => permit,
            Err(failure) => {
                let disposition = failure.disposition();
                if disposition == Disposition::Refused {
                    self.inner.refused.fetch_add(1, Ordering::Relaxed);
                }
                return self.unrun(started, task_name, disposition, failure.into_error(), run);
            }
        };
        // Dropped on every exit path from here on, including a future the caller
        // drops while this run is suspended: the budget returns to what it was,
        // the in-flight count falls, and no body outlives the call.
        let _admission = self.charge(permit);
        // The run enters the world here, through the journal, before its body
        // exists (ADR 0006 R3). A feed that cannot record it refuses the run
        // rather than letting the world miss one that happened.
        let mut world = match WorldRun::admit(self.inner.runs.as_ref()) {
            Ok(world) => world,
            Err(error) => {
                return self.refuse(
                    started,
                    task_name,
                    format!("the world's run feed refused this run: {error}"),
                );
            }
        };

        // The root budget is charged here, after the run was admitted and confirmed
        // to have authority, but before the body runs. A repair is charged exactly
        // like any other attempt — that is what makes an authorized repair *distinct*
        // rather than a reset (T13). The refusal is reported as a terminal report
        // rather than an error, because the run genuinely happened and its state is
        // exactly this.
        if let Some(refusal) = self.root_budget_refusal(run, repair, spend).await {
            world.settle(Disposition::Refused);
            let error = FlowError::failed(format_args!(
                "the run's root budget refused this attempt: {refusal}"
            ));
            return self.unrun(started, task_name, Disposition::Refused, error, run);
        }

        let trail = Trail::new(self.inner.limits.progress_capacity());
        let root = self.rooted(&trail, run, grant_delta, &recorded_definition);
        let scope = match root.enter(task_name.as_str()) {
            Ok(scope) => scope,
            // The task's own step could not be entered: the name validated at
            // declaration, so this is the depth ceiling or a stop that arrived
            // in the instant between admission and entry. Either way the body
            // never ran, and the report says so rather than inventing a trail.
            Err(error) => {
                let disposition = disposition_of(&error);
                world.settle(disposition);
                return self.unrun(started, task_name, disposition, error, run);
            }
        };
        let entry = Entry {
            started,
            task: task_name,
            run,
            trail,
        };
        self.finish(task, input, scope, repair, &mut world, entry)
            .await
    }

    /// Run the body of an entered run under the budget of the hosts above it,
    /// and assemble the report of how it ended.
    async fn finish<I, O, F, Fut>(
        &self,
        task: &Task<F>,
        input: I,
        scope: Scope,
        repair: Option<&RepairTicket>,
        world: &mut WorldRun,
        entry: Entry,
    ) -> Report<O>
    where
        F: Fn(Scope, I) -> Fut,
        Fut: Future<Output = Result<O, FlowError>>,
    {
        let Entry {
            started,
            task: task_name,
            run,
            trail,
        } = entry;
        // A nested run's identity joins the set its parent established, so a
        // child two levels down sees every ancestor's host and is charged the
        // same way. Sibling runs have their own scopes and their own sets.
        //
        // Outside a scope there is no held set, and an empty one is the correct
        // reading: this run holds nothing it inherited, so it is charged for
        // everything it does itself. `Vec::new()` rather than a discard of the
        // not-entered error, which carries no permit information at all.
        let held = match HELD_PERMITS.try_with(Clone::clone) {
            Ok(held) => held,
            Err(not_entered) => {
                lgwks_std::trace::debug!(
                    ?not_entered,
                    "submit: no enclosing scope holds a permit set"
                );
                Vec::new()
            }
        };
        let charged = if held.contains(&self.inner.identity) {
            held
        } else {
            let mut charged = held;
            charged.push(self.inner.identity);
            charged
        };
        let body = (task.body)(scope.clone(), input);
        let deadline = self.inner.limits.default_deadline();
        // Everything from here to the report is one `Box`: this frame is on the
        // stack of every *nested* run, and depth-four nesting at an admission
        // ceiling of one is a shipped property (T04). Holding the body future,
        // both task-local scopes, the deadline wrapper and the report assembly
        // inline makes each level's frame large enough that the default test
        // thread's stack overflows at the depths the invariant names. One
        // pointer per level is the difference between that and not.
        // The run's store, definition and authority all ride on `scope`, so a
        // durable step records against *this* run and a step on a spawned task
        // keeps all three (#909). With no store the scope carries none, which is
        // what makes "no durability" the default rather than a claim. The
        // held-permit set is the one task-local left: it is read at admission,
        // not at a step, and it is boxed so a run nested in another run's body
        // costs the parent a pointer rather than its frame.
        let outcome =
            Box::pin(HELD_PERMITS.scope(charged, within(&scope, BODY_STEP, deadline, body))).await;

        // The task's own step settles before the ring is read, so the trail's
        // first entry says how the run ended as every later entry does.
        scope.settle(&outcome);
        let snapshot = trail.snapshot();
        let (disposition, output, error, needs) = read_outcome(outcome, &scope);
        // A blocked run mints its repair ticket from the same shortfall the report
        // carries, and only when the host holds a ledger to decide a later repair
        // against. A blocked run *inside* a repair mints none: a repair that could
        // not close the shortfall hands back the run it was given rather than a
        // fresh ticket for the same block, which would be a way to retry a denied
        // repair by asking again.
        let repair = match needs.as_ref() {
            Some(deficit) if repair.is_none() => self.ticket_for(run, deficit),
            Some(_) | None => None,
        };
        world.settle(disposition);
        self.report(Terminal {
            started,
            task: task_name,
            disposition,
            output,
            error,
            trail: TrailSnapshot::Taken(snapshot),
            run,
            needs,
            repair,
        })
    }

    /// A run whose body never ran and whose step trail is therefore empty.
    ///
    /// The one assembly point for the two reports that describe a run the host
    /// declined before a body could start — [`refuse`](Self::refuse) and
    /// [`blocked`](Self::blocked) — because the parts they share are facts rather
    /// than style: no output exists, no step was entered, and the failure is
    /// located at the task because there is no step to locate it in. The two
    /// differ only in their disposition, their run id, and whether unmet
    /// authority rides along.
    pub(super) fn not_admitted<O>(
        &self,
        started: Instant,
        task: TaskName,
        declined: Declined,
    ) -> Report<O> {
        let Declined {
            disposition,
            reason,
            run,
            needs,
            repair,
        } = declined;
        let at = Arc::from(task.as_str());
        self.report(Terminal {
            started,
            task,
            disposition,
            output: None,
            error: Some(FlowError::Failed { at, reason }),
            trail: TrailSnapshot::Empty,
            run,
            needs,
            repair,
        })
    }

    /// A resume refused because its run, or the host's table of resumed runs, is
    /// full: `Refused`, no body, no step, no permit and no trail, carrying the
    /// run it was asked for so a caller can try it again.
    fn claim_refused<O>(
        &self,
        started: Instant,
        task: TaskName,
        run: RunId,
        refusal: ClaimRefused,
    ) -> Report<O> {
        lgwks_std::trace::debug!(
            run = %run.id().to_hex(),
            tenant = self.inner.tenant.as_str(),
            ?refusal,
            "execute: a resume was refused its claim on the run"
        );
        self.inner.refused.fetch_add(1, Ordering::Relaxed);
        self.report(Terminal {
            started,
            task,
            disposition: Disposition::Refused,
            output: None,
            error: Some(refusal.into_error(run)),
            trail: TrailSnapshot::Empty,
            run: Some(run),
            needs: None,
            repair: None,
        })
    }

    /// A report for a run the host declined before its body: no output, no step
    /// entered, no trail, and no need or repair ticket riding along.
    fn unrun<O>(
        &self,
        started: Instant,
        task: TaskName,
        disposition: Disposition,
        error: FlowError,
        run: Option<RunId>,
    ) -> Report<O> {
        self.report(Terminal {
            started,
            task,
            disposition,
            output: None,
            error: Some(error),
            trail: TrailSnapshot::Empty,
            run,
            needs: None,
            repair: None,
        })
    }

    /// Why a resume of `run` is refused because the store knows it as another
    /// tenant's, when it is.
    ///
    /// A run the store has *never seen* is not refused: a process killed before
    /// its first record left no evidence at all, and refusing that resume would
    /// make the exactly-at-least-once step unrecoverable. A store that cannot
    /// say who owns the run has not said it is this tenant's, so the adoption is
    /// refused rather than assumed.
    fn foreign_resume(&self, run: RunId) -> Option<String> {
        let store = self.inner.records.as_ref()?;
        match store.owner(run) {
            Ok(Some(owner)) if owner != self.inner.tenant.as_str() => Some(
                StoreError::ForeignTenant {
                    owner,
                    asked: self.inner.tenant.as_str().to_owned(),
                }
                .to_string(),
            ),
            Ok(Some(_) | None) => None,
            Err(unread) => Some(unread.to_string()),
        }
    }

    /// The refusal of a resume whose declared definition drifted from the one
    /// its records carry, or whose store could not be read to check.
    ///
    /// Only a *declared* identity is checked: the recorded one is what the steps
    /// write and look up under, so comparing a run's records against it would
    /// agree by construction and fence nothing. Every refusal is refused; only
    /// the *kind* differs, and the kind is the store's own. A drift is the flow's
    /// typed `Incompatible` carrying its axis; every other store refusal reaches
    /// the caller as the store's own error. Matching on `Incompatible` alone once
    /// let a read failure through as though the check had passed (INV-BOT-7).
    fn drifted(
        &self,
        resume: Option<RunId>,
        declared: Option<&DefinitionIdentity>,
        task: &TaskName,
    ) -> Option<FlowError> {
        let (Some(run), Some(store), Some(declared)) =
            (resume, self.inner.records.as_ref(), declared)
        else {
            return None;
        };
        admits_definition(store, run, declared)
            .err()
            .map(|error| error.located_at(&Arc::from(task.as_str())))
    }

    /// Why a run cannot proceed for want of a store or an identity, when it
    /// cannot.
    ///
    /// A resume on a host with nothing to replay from would run every step again
    /// under a caller's belief that finished steps are kept. A host given a store
    /// promised durable steps, and a build that cannot mint the identity those
    /// steps are keyed by refuses rather than running them unrecorded.
    fn storeless(&self, resume: Option<RunId>, run: Option<RunId>) -> Option<String> {
        match (self.inner.records.is_some(), resume, run) {
            (false, Some(adopted), _) => Some(format!(
                "this host has no run store, so run {} has nothing to resume from",
                adopted.id().to_hex()
            )),
            (true, _, None) => Some(String::from(
                "this host has a run store but could not mint a run identity \
                 (the `ephemeral` feature is off or its entropy source failed); \
                 no step ran unrecorded",
            )),
            (false, None, _) | (true, _, Some(_)) => None,
        }
    }

    /// The needs of a task that neither the host's grants nor this run's repair
    /// delta cover, from one complete pass.
    ///
    /// The base grant and the delta are checked *together* and never one
    /// replacing the other: a repair adds authority for one run, and every
    /// capability the host already had still holds.
    fn shortfall(&self, needs: &[Cap], task: &TaskName, grant_delta: &GrantSet) -> Option<Deficit> {
        let covers = |cap: &Cap| self.inner.grants.grants(cap) || grant_delta.grants(cap);
        Deficit::from_shortages(uncovered(needs, covers, Some(&Demand::new(task.as_str()))))
    }

    /// Why the run's root budget refuses this attempt, when it does.
    ///
    /// Charged after the run was admitted and confirmed to have authority, but
    /// before the body runs. A repair is charged exactly like any other attempt,
    /// which is what makes an authorized repair *distinct* rather than a reset
    /// (T13).
    async fn root_budget_refusal(
        &self,
        run: Option<RunId>,
        repair: Option<&RepairTicket>,
        spend: u64,
    ) -> Option<String> {
        let run = run?;
        let ledger = self.inner.ledger.as_ref()?;
        ledger
            .charge(
                self.inner.tenant.as_str(),
                run,
                repair.map(RepairTicket::stamp).as_ref(),
                spend,
                self.inner.repair_attempts,
                self.inner.repair_spend,
            )
            .await
            .err()
            .map(|refusal| refusal.to_string())
    }

    /// The root scope of a run: the host's own clock, not a fresh one, so a
    /// run's admission wait and its body's `within` budget are two readings of
    /// the same timeline, and the host's policy, so there is one shared retry
    /// budget rather than one per run.
    fn rooted(
        &self,
        trail: &Arc<Trail>,
        run: Option<RunId>,
        grant_delta: &GrantSet,
        recorded: &DefinitionIdentity,
    ) -> Scope {
        Scope::rooted(
            self.inner.tenant.clone(),
            self.inner.token.child_token(),
            Arc::clone(trail),
            self.inner.clock.clone(),
            run,
            Arc::clone(&self.inner.policy),
            self.inner.max_pending,
        )
        .hosted(
            self.authority(grant_delta),
            self.records(run),
            Arc::new(recorded.clone()),
        )
    }

    /// The repair ticket for a run short of `deficit`: only for a run whose
    /// epoch the ledger knows, so a host without a ledger mints none.
    fn ticket_for(&self, run: Option<RunId>, deficit: &Deficit) -> Option<RepairTicket> {
        let (Some(run), Some(ledger)) = (run, self.inner.ledger.as_ref()) else {
            return None;
        };
        Some(RepairTicket::of_deficit(
            run,
            self.inner.tenant.as_str(),
            ticket_epoch(ledger.control(run)),
            deficit,
        ))
    }

    /// A run refused before admission: `Refused`, no body, no step, no run id,
    /// and the reason located at the task.
    pub(super) fn refuse<O>(&self, started: Instant, task: TaskName, reason: String) -> Report<O> {
        self.inner.refused.fetch_add(1, Ordering::Relaxed);
        self.not_admitted(started, task, Declined::plain(Disposition::Refused, reason))
    }

    /// A run blocked on unmet authority: `Blocked`, no body, no step, every unmet
    /// need, and — when this host has a ledger to decide a repair against — the
    /// repair ticket naming exactly those needs.
    ///
    /// One assembly point for the two facts a caller acts on, because they come
    /// from the same [`Deficit`] and a report that named a different set in its
    /// needs and its ticket would be the one thing a caller could not detect.
    pub(super) fn blocked<O>(
        &self,
        started: Instant,
        task: TaskName,
        run: Option<RunId>,
        deficit: Deficit,
    ) -> Report<O> {
        // A ticket is minted only for a run whose epoch the ledger knows and for a
        // run that is *not* already carrying a repair: a repair that could not
        // close the shortfall hands back the run it was given rather than minting a
        // fresh ticket for the same block, which would be a way to retry a denied
        // repair by asking again.
        let repair = self.ticket_for(run, &deficit);
        self.not_admitted(
            started,
            task,
            Declined {
                disposition: Disposition::Blocked,
                reason: String::from("this host's authority does not cover what this task needs"),
                run,
                needs: Some(deficit),
                repair,
            },
        )
    }

    /// The run identity for a run, given the one a resume supplied.
    ///
    /// A resume keeps its own identity. A *fresh* run mints one only when a store
    /// is installed **and** the crate was built with the `ephemeral` feature,
    /// which is the estate's one entropy source. Without a store there is
    /// nothing to key a record by, so a minted id would be an identity nothing
    /// is stored under; without `ephemeral` there is no source to mint from, and
    /// INV-RANDOM-1 refuses a cheaper substitute. Without a store the
    /// run proceeds with no run id; with a store and no identity, `execute`
    /// refuses rather than run steps the store promised to keep.
    pub(super) fn adopt_run(&self, resume: Option<RunId>) -> Option<RunId> {
        match (self.inner.records.is_some(), resume) {
            (true, Some(run)) => Some(run),
            (true, None) => self.mint_run(),
            (false, _) => None,
        }
    }

    /// A fresh run identity, or `None` when this build cannot mint one.
    ///
    /// The `ephemeral` gate is not incidental: it is the only feature that turns
    /// on the estate's entropy source, and a run id derived from a clock or a
    /// counter would make two runs indistinguishable — exactly what
    /// `lgwks_std::random`'s invariant exists to refuse.
    #[cfg(feature = "ephemeral")]
    pub(super) fn mint_run(&self) -> Option<RunId> {
        RunId::mint().ok()
    }

    /// Without the entropy feature there is no source to mint from.
    ///
    /// A build that wants a durable fresh run enables `ephemeral`; a build that
    /// only resumes supplied run ids needs nothing. Returning `None` keeps the
    /// report honest rather than minting an identity the crate cannot vouch for.
    #[cfg(not(feature = "ephemeral"))]
    pub(super) fn mint_run(&self) -> Option<RunId> {
        None
    }

    /// The authority this run's steps consult: the host's grant plus the delta of
    /// any repair this run carries.
    ///
    /// Always attached to the run's root scope, including for a host that grants
    /// nothing. That is what lets the registry be resolved, and a step under a
    /// host that grants nothing is refused for whatever it asks about, exactly as
    /// a step under no host is.
    pub(super) fn authority(&self, grant_delta: &GrantSet) -> RecordsAuthority {
        RecordsAuthority(Arc::new(RunAuthority {
            base: self.inner.grants.clone(),
            delta: grant_delta.clone(),
            domains: self.inner.domains,
        }))
    }

    /// The erased record store for this run, when there is one.
    pub(super) fn records(&self, run: Option<RunId>) -> Option<Records> {
        // A store with no run cannot key a record, and a run with no store has
        // nowhere to put one; both pairs mean the same thing — this run is not
        // durable — so they are refused here rather than at the first step.
        let store = self.inner.records.as_ref()?;
        run?;
        Some(store.clone())
    }
}

/// One admitted run's place in the world's feed, settled exactly once.
///
/// A guard rather than a call at each exit: a run whose future the caller drops
/// while it is suspended still settles, as [`RunOutcome::Aborted`], so the world
/// never holds an admitted run nobody will settle.
struct WorldRun(Option<(RunFeed, RunTicket)>);

impl WorldRun {
    /// Admit into `feed` when the host has one; a host without a feed admits
    /// into nothing.
    fn admit(feed: Option<&RunFeed>) -> Result<Self, RunFeedError> {
        match feed {
            Some(feed) => Ok(Self(Some((feed.clone(), feed.admit()?)))),
            None => Ok(Self(None)),
        }
    }

    /// Settle the run as `disposition` says it ended.
    fn settle(&mut self, disposition: Disposition) {
        self.record(match disposition {
            Disposition::Succeeded => RunOutcome::Succeeded,
            Disposition::Failed => RunOutcome::Failed,
            Disposition::Cancelled => RunOutcome::Cancelled,
            Disposition::DeadlineExceeded => RunOutcome::TimedOut,
            Disposition::Refused => RunOutcome::Refused,
            Disposition::Blocked => RunOutcome::Blocked,
            Disposition::Parked => RunOutcome::Parked,
        });
    }

    /// Write the settlement once. The report the caller gets is already
    /// decided, so a journal that refuses the settlement is traced, and the
    /// replay reads the run as admitted and unsettled, which is what it is.
    fn record(&mut self, outcome: RunOutcome) {
        if let Some((feed, ticket)) = self.0.take()
            && let Err(error) = feed.settle(&ticket, outcome)
        {
            lgwks_std::trace::warn!(%error, "execute: the world's run feed refused a settlement");
        }
    }
}

impl Drop for WorldRun {
    fn drop(&mut self) {
        self.record(RunOutcome::Aborted);
    }
}

/// What an entered run carries from admission to its report.
struct Entry {
    /// When the call began, for the report's elapsed time.
    started: Instant,
    /// The task's name.
    task: TaskName,
    /// The run's identity, when the host keeps one.
    run: Option<RunId>,
    /// The ring the run's steps are recorded in.
    trail: Arc<Trail>,
}

/// How a finished body reads: its disposition, output, located error and, for a
/// blocked run, the shortfall that blocked it.
///
/// A step that reached for authority it does not have blocks the run, and its
/// own shortfall rides out with it: the report's needs and the repair ticket are
/// both built from this one value, so they cannot disagree about what the run was
/// missing. Every other failure is an ordinary located error.
fn read_outcome<O>(
    outcome: Result<O, FlowError>,
    scope: &Scope,
) -> (Disposition, Option<O>, Option<FlowError>, Option<Deficit>) {
    match outcome {
        Ok(value) => (Disposition::Succeeded, Some(value), None, None),
        Err(error) => {
            let located = error.located(scope);
            match located.deficit().cloned() {
                Some(deficit) => (Disposition::Blocked, None, Some(located), Some(deficit)),
                None => (disposition_of(&located), None, Some(located), None),
            }
        }
    }
}

/// The repair epoch a ticket for a run names: the run's current epoch, or zero
/// for a run the ledger has never seen, because zero is the epoch of a run no
/// repair has touched (`Control::epoch`).
pub(super) fn ticket_epoch(control: Option<Control>) -> u64 {
    match control {
        Some(control) => control.epoch(),
        None => 0,
    }
}

/// Whether `declared` may resume `run`, judged against the definition the run's
/// records were first written under.
///
/// The host's pre-flight reads the recorded definition rather than making a step
/// read: a step read is the durable step's own question, and a fault on it must
/// reach the step that asked, as the store's failure (INV-BOT-59), not be
/// spent here and reported as a refusal of the whole run. A run the store has
/// never seen is not refused; its first step writes the identity.
///
/// # Errors
///
/// [`FlowError::Incompatible`] naming the drift's axis, or whatever the store
/// reports when it cannot name the recorded definition.
fn admits_definition(
    store: &Records,
    run: RunId,
    declared: &DefinitionIdentity,
) -> Result<(), FlowError> {
    // A run the store has never seen has no recorded definition to drift from.
    let drift = store
        .recorded_definition(run)?
        .and_then(|recorded| declared.drift_from(&recorded));
    if let Some(found) = drift {
        let refusal = Err(FlowError::incompatible("", found));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "admits_definition: returning an error to the caller");
        return refusal;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cap::Cap;
    use crate::effect::RunId;
    use crate::gate::GrantSet;
    use crate::task::{Host, name::TaskName};

    /// What this module's tests report.
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A storeless host for `acme` holding `grants`.
    fn host(grants: GrantSet) -> Result<Host, Box<dyn std::error::Error>> {
        Ok(Host::builder("acme")?.grants(grants).build()?)
    }

    /// A storeless host refuses a resume, since it has nothing to replay, and
    /// lets a fresh run through whether or not it minted an identity.
    #[test]
    fn execute_a_storeless_host_refuses_only_a_resume() -> TestResult {
        let host = host(GrantSet::empty())?;
        let run = RunId::from_hex(&format!("1{}", "0".repeat(31)))?;
        assert!(
            host.storeless(Some(run), Some(run)).is_some(),
            "a resume with no store is refused"
        );
        assert!(
            host.storeless(None, None).is_none(),
            "a fresh run with no store and no identity proceeds"
        );
        assert!(
            host.storeless(None, Some(run)).is_none(),
            "a fresh run with no store proceeds"
        );
        Ok(())
    }

    /// The host's grants and the run's repair delta cover a need together;
    /// either alone covers what it grants, and a need neither grants is short.
    #[test]
    fn execute_grants_and_the_repair_delta_cover_together() -> TestResult {
        let host = host(GrantSet::empty().grant(Cap::net()))?;
        let task = TaskName::new("probe")?;
        let empty = GrantSet::empty();
        let fs = GrantSet::empty().grant(Cap::fs());
        assert!(
            host.shortfall(&[Cap::net()], &task, &empty).is_none(),
            "the host's own grant covers its need"
        );
        assert!(
            host.shortfall(&[Cap::fs()], &task, &fs).is_none(),
            "the repair delta covers its need"
        );
        assert!(
            host.shortfall(&[Cap::net(), Cap::fs()], &task, &fs)
                .is_none(),
            "the two together cover both needs"
        );
        assert!(
            host.shortfall(&[Cap::fs()], &task, &empty).is_some(),
            "a need neither grants is short"
        );
        Ok(())
    }
}
