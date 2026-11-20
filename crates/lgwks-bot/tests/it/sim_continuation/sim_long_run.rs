//! The long-run family: a hundred-plus real continuations lose and duplicate no
//! effect, and the fold agrees with the oracle at every generation.

use super::*;

/// One long run's moving parts, so a batch is one call and every rung of it is
/// a named step rather than a chain of fallible expressions.
struct LongRun {
    /// The live generation.
    journal: FileJournal,
    /// What the run meant to do, attempt by attempt.
    oracle: Oracle,
    /// The seeded fate of every attempt.
    schedule: Schedule,
    /// Attempts left unknown, oldest first, waiting for evidence.
    stranded: VecDeque<(u64, u64, EffectKey)>,
    /// The batch being filled, reused across batches.
    batch: Vec<EffectEvent>,
    /// Attempts since the action last turned over.
    since_turn: u64,
    /// Continuations reaped so far.
    continuations: u64,
}

impl LongRun {
    /// Two concurrent attempts per action, every batch, in every band.
    ///
    /// A run whose concurrency the seed chooses reaps a different number of
    /// continuations from the same number of batches, which would make this
    /// family's wall clock a property of the seed rather than of the lifecycle;
    /// the concurrency itself varies across the other families, where it is not
    /// the thing being bounded.
    const ACTIONS: u64 = 2;

    /// Fill one batch, flush it, and continue when the watermark says so.
    ///
    /// One flush for the batch, which is the shipped group-commit door and what a
    /// controller with sixteen settled things to record actually does.
    fn step(&mut self) -> TestResult {
        self.batch.clear();
        for _ in 0..ATTEMPTS_PER_BATCH {
            self.admit_one()?;
        }
        self.reconcile();
        self.journal.compare_and_append_all(&self.batch)?;
        let Some(successor) = continue_if_due(&mut self.journal)? else {
            return Ok(());
        };
        assert!(
            self.stranded.len() <= MAX_CHECKPOINT_UNRESOLVED,
            "a run holding {} unresolved attempts is past the declared carry \
             bound of {MAX_CHECKPOINT_UNRESOLVED}, and such a continuation is \
             refused rather than truncating the carry",
            self.stranded.len()
        );
        self.journal = successor;
        self.continuations = self.continuations.saturating_add(1);
        // The oracle has already settled everything the ladder settled, so the
        // fold the successor can produce is checked against it at every
        // generation rather than once at the end: a disagreement that only
        // appears at generation ninety is caught there.
        assert_fold_agrees(&self.journal, &self.oracle, 1)
    }

    /// Add one attempt's ladder to the batch, under the seeded fate.
    fn admit_one(&mut self) -> TestResult {
        let fate = self.schedule.next();
        self.since_turn = self.since_turn.saturating_add(1);
        let action = if self.since_turn > Self::ACTIONS {
            self.since_turn = 1;
            1
        } else {
            self.since_turn
        };
        let next = self.oracle.dispatched.saturating_add(1);
        let rungs = ladder(fate, action, next)?;
        self.batch.extend(rungs);
        if fate == Fate::Unknown {
            let key = key_of(action, next)?;
            self.stranded.push_back((action, next, key));
        }
        self.oracle.dispatch(action, next, fate);
        Ok(())
    }

    /// Evidence settles the oldest carried attempts, in the batch about to flush.
    ///
    /// That is what keeps the in-flight set bounded: a controller that never
    /// reconciles is not a long run, it is an unbounded queue, and the carry bound
    /// says so rather than dropping anything. The evidence rides in the batch's own
    /// flush, as a controller that reconciles while it works writes it, rather than
    /// paying one `sync_all` per settled attempt.
    fn reconcile(&mut self) {
        while self.stranded.len() > LOW_WATER {
            let Some((action, number, key)) = self.stranded.pop_front() else {
                break;
            };
            self.batch.push(EffectEvent::OutcomeObserved {
                key,
                evidence: EffectEvidence::Applied,
            });
            self.oracle.settle(action, number);
        }
    }
}

/// A long run loses and duplicates no effect, through a hundred continuations.
///
/// The oracle is the point: it records what the run *meant* to do, attempt by
/// attempt, and every one of those attempts is then checked three ways — that the
/// fold still knows its fate at every generation, that a re-presentation is refused
/// everywhere, and that the live journal's own chain verifies to its own tail.
fn long_run(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("long")?;
        let path = dir.join("run.jrnl");
        let batches = 230 + u64::from(sim.rng().below(20));
        let journal = FileJournal::open_continuing_with(&path, policy()?)?;
        let mut run = LongRun {
            journal,
            oracle: Oracle::default(),
            schedule: Schedule::new(sim),
            stranded: VecDeque::new(),
            batch: Vec::new(),
            since_turn: 0,
            continuations: 0,
        };
        for _ in 0..batches {
            run.step()?;
        }
        let LongRun {
            mut journal,
            oracle,
            stranded,
            continuations,
            ..
        } = run;
        let stranded_at_end = stranded.len();

        assert!(
            continuations >= 100,
            "a run of {} attempts reached only {continuations} continuations, which is \
             not the hundred the lifecycle claim rests on",
            oracle.dispatched
        );
        assert_fold_agrees(&journal, &oracle, 1)?;

        // Every attempt the run made is refused a second time, which is what "no
        // duplicated effect" means at the journal's boundary: the ladder refuses
        // the folded key and the fold refuses the older ones.
        for (action, number) in oracle.attempts.keys().copied() {
            let refused = journal.compare_and_append(
                journal.tail(),
                &EffectEvent::IntentAdmitted {
                    key: key_of(action, number)?,
                },
            );
            assert!(
                refused.is_err(),
                "attempt {number} of action {action} was admitted a second time: \
                 {refused:?}"
            );
        }
        let live = journal.path().to_path_buf();
        drop(journal);

        let reopened = FileJournal::open_active(&path)?;
        assert_eq!(
            reopened.path(),
            live.as_path(),
            "the generation walk reaches the journal the run ended on"
        );
        // From its own base, not from the genesis: the seal frame the successor
        // was opened from is not an event and is not in its committed entries, so
        // folding from the genesis would report a disagreement at the first entry.
        let entries = reopened.committed_entries()?;
        assert_eq!(
            lgwks_bot::journal::verify_chain_from(reopened.chain_from(), &entries)?,
            reopened.tail(),
            "and the live journal's own chain verifies from its own base to its tail"
        );
        sim.record("long-run-no-loss-no-duplicate");
        sim.trace.record_number("long-attempts", oracle.dispatched);
        sim.trace.record_number("long-continuations", continuations);
        sim.trace.record_number("long-unknown", oracle.unknown());
        sim.trace
            .record_number("long-still-unknown", stranded_at_end);
        Ok(())
    })
}

/// The long run, over a two-seed band rather than the eight-seed one.
///
/// Declared as its own test instead of through [`band_family`] because its band
/// is not the standard one, for the measured reason in the module note: one sweep
/// of eight seeds leaves about 1,840 pool threads alive. A two-seed band keeps the
/// property under test -- a hundred continuations, the oracle checked at every
/// generation, both sweeps replaying to one hash -- at a fifth of the handle
/// count; the other four families carry the rest of the seed space.
#[test]
fn long_run_two_seeds_replay_a_hundred_continuations() -> TestResult {
    long_run(Band::new(0, 2))
}
