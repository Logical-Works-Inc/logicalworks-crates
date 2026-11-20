//! Simulation family: continue-as-new, driven by a seeded sweep.
//!
//! Every scenario drives the **real** [`FileJournal`] — its real storage-owner
//! thread, frame codec, chain verification and `recover` fold — through dozens of
//! real continuations. Nothing here reimplements a journal or a checkpoint: a
//! copy would pass while the shipped one rotted.
//!
//! The seed controls how many attempts the run makes, how many actions they
//! belong to, how many end unknown, and how many tenants share the directory.
//! Every run satisfies the same properties whatever the seed chose, and replays
//! to the same trace hash.
//!
//! The properties, one family each:
//!
//! | Family | Property it pins |
//! |---|---|
//! | `long_run` | a hundred-plus continuations lose and duplicate no effect, and the fold agrees with the oracle at every generation |
//!
//! `long_run` is declared over one band, and the reason is measured. A hundred
//! continuations is two hundred journal handles, each a blocking-pool thread the
//! pool keeps for ten idle seconds after it drops: one band measures 4.2 s alone,
//! 127 s beside a second and 218 s for four on the 15-core host, which is the
//! pool and the APFS commit path, not the lifecycle.
//!
//! The other four families are declared over two bands each for the same reason,
//! so the family stays inside the gate's wall-clock budget. The seed space is
//! not abandoned: `unresolved_carry`, `tenant_continuation`, `bounded_reopen`
//! and `oracle_matches` still sweep sixteen seeds each, and every family
//! replays each seed it runs.
//! | `unresolved_carry` | every attempt left unknown is unknown in the successor and settles there, never resent |
//! | `tenant_continuation` | two tenants over one directory continue independently and lose nothing |
//! | `bounded_reopen` | a successor's own size does not grow with the history behind it |
//! | `oracle_matches` | the fold after a *reopen* is exactly what the oracle recorded |
//!
//! Bands partition the seed space so no seed is untested and a failure names its
//! band. Each declared test sweeps its band twice and requires the two trace hashes
//! to be identical, so a nondeterministic run fails even when every assertion
//! happens to pass.

use crate::sim;

// The band-declaration macro, defined once for the whole layer.
use crate::band_family;

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;

use lgwks_bot::effect::{
    ActionDigest, ActionId, AttemptId, EffectIdentity, EffectKey, EnvironmentEpoch, EnvironmentId,
    FlowRevision, Id128, RunId,
};
use lgwks_bot::journal::{
    AttemptStatus, ContinuationPolicy, EffectEvent, EffectEvidence, EffectJournal, FileJournal,
    MAX_CHECKPOINT_UNRESOLVED, Verification, VerificationResult,
};

use sim::Band;

/// A test that needs a parsed value returns `Result` and propagates, because the
/// crate forbids a panicking path anywhere, tests included.
pub(crate) type TestResult = Result<(), Box<dyn Error>>;

/// The run every key in this family belongs to.
const RUN: &str = "0a0b0c0d0e0f10111213141516171819";
/// The environment every key is fenced against.
const ENV: &str = "2122232425262728292a2b2c2d2e2f30";
/// The flow revision every key was produced from.
const FLOW_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
/// The digest every key binds.
const DIGEST_HEX: &str = "1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100";
/// The predicate every verification names, as 32 hex characters.
const PREDICATE: &str = "0f0e0d0c0b0a09080706050403020100";
/// The observations every verification is decided over.
const OBSERVATIONS: &[u8] = b"the continuation simulation's own observations";

/// The trigger this family drives.
///
/// Small enough that a long run reaches a hundred continuations inside a test's
/// wall clock, and far below the shipped ceilings so the files it writes never
/// approach the frame bound. `the_shipped_watermark_leaves_the_declared_headroom`
/// in `journal_continuation.rs` pins the shipped numbers, and
/// `examples/journal_continuation.rs` drives hundreds of real continuations at a
/// much larger trigger.
const SIM_EVENT_WATERMARK: u64 = 96;
const SIM_BYTE_WATERMARK: u64 = 8 * 1024 * 1024;

/// How many attempts every family commits per flush.
///
/// Sixteen, so a hundred continuations are reachable inside a test's wall clock
/// without a per-attempt `fsync`: one flush per batch is the shipped group-commit
/// door, and it is what makes the run a claim about the *lifecycle* rather than
/// about the device's flush rate. Measured: at one flush per attempt these
/// families took 50–120 s each inside the full suite.
pub(crate) const ATTEMPTS_PER_BATCH: u64 = 16;

/// The in-flight set a long run reconciles down to before it continues again.
///
/// Well under [`MAX_CHECKPOINT_UNRESOLVED`], so the carry bound is a backstop the
/// run never reaches rather than the thing that paces it.
const LOW_WATER: usize = 16;

/// The policy this family opens with.
fn policy() -> Result<ContinuationPolicy, Box<dyn Error>> {
    Ok(ContinuationPolicy::new(
        SIM_EVENT_WATERMARK,
        SIM_BYTE_WATERMARK,
    )?)
}

/// The action every attempt of `which` belongs to, numbered from one.
///
/// The tenants take their numbers from the top of the `u64` range, so a tenant's
/// action and an ordinary action can never be the same identifier.
pub(crate) fn action_of(which: u64) -> Result<ActionId, Box<dyn Error>> {
    Ok(ActionId::from_hex(&format!("{which:032x}"))?)
}

/// The number `action_of` was given, read back out of the key.
///
/// The oracle is keyed by the number the seed chose and the fold is keyed by the
/// identifier the journal holds, so something has to convert between them. Reading
/// it back out of the key rather than carrying a second table is what makes the
/// comparison a fact about the journal's own identity.
pub(crate) fn action_number(action: ActionId) -> Result<u64, Box<dyn Error>> {
    let hex = action.id().to_hex();
    let tail = hex
        .get(hex.len().saturating_sub(16)..)
        .ok_or("a 128-bit id renders as 32 hex characters")?;
    Ok(u64::from_str_radix(tail, 16)?)
}

/// A key for `attempt` of `action`, under the shared identity.
pub(crate) fn key_of(action: u64, attempt: u64) -> Result<EffectKey, Box<dyn Error>> {
    let run = RunId::from_hex(RUN)?;
    let environment = EnvironmentId::from_hex(ENV)?;
    let flow = FlowRevision::from_tagged("blake3_256", FLOW_HEX)?;
    let action = action_of(action)?;
    let attempt = AttemptId::from_decimal(&attempt.to_string())?;
    let digest = ActionDigest::from_tagged("blake3_256", DIGEST_HEX)?;
    let epoch = EnvironmentEpoch::from_decimal("1")?;
    Ok(EffectIdentity::new(run, environment, flow).key(action, attempt, digest, epoch))
}

/// A verification decided at `version`, with the answer this family's fate says.
pub(crate) fn verdict(version: u64, satisfied: bool) -> Result<Verification, Box<dyn Error>> {
    Ok(Verification::new(
        Id128::from_hex(PREDICATE)?,
        version,
        lgwks_std::hash::blake3(OBSERVATIONS),
        if satisfied {
            VerificationResult::Satisfied
        } else {
            VerificationResult::NotSatisfied
        },
    ))
}

/// One attempt's whole ladder, with the outcome its fate names.
pub(crate) fn ladder(
    fate: Fate,
    action: u64,
    attempt: u64,
) -> Result<Vec<EffectEvent>, Box<dyn Error>> {
    let key = key_of(action, attempt)?;
    let mut events = vec![
        EffectEvent::IntentAdmitted { key },
        EffectEvent::DispatchPrepared { key },
    ];
    match fate {
        Fate::Unknown => {}
        Fate::NotApplied => events.push(EffectEvent::OutcomeObserved {
            key,
            evidence: EffectEvidence::NotApplied,
        }),
        Fate::Applied => {
            events.push(EffectEvent::OutcomeObserved {
                key,
                evidence: EffectEvidence::Applied,
            });
            events.push(EffectEvent::Verified {
                key,
                verification: verdict(1, true)?,
            });
        }
        Fate::VerificationFailed => {
            events.push(EffectEvent::OutcomeObserved {
                key,
                evidence: EffectEvidence::Applied,
            });
            events.push(EffectEvent::Verified {
                key,
                verification: verdict(2, false)?,
            });
        }
    }
    Ok(events)
}

/// How an attempt ended, as the oracle records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fate {
    /// Dispatched, and the outcome was never observed.
    Unknown,
    /// Dispatched, and the evidence says it landed.
    Applied,
    /// Dispatched, and the evidence says it did not land.
    NotApplied,
    /// Applied, and a predicate was evaluated and did not hold.
    VerificationFailed,
}

/// What the oracle holds about one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Record {
    /// The attempt's attempt number, which is what the ladder is ordered by.
    pub attempt: u64,
    /// What happened to it.
    pub fate: Fate,
}

/// The oracle: what the run believes happened, independent of the journal.
///
/// A `BTreeMap` keyed by `(action, attempt)` so "the latest attempt of this action"
/// is a question the oracle answers the same way the journal's fold does, rather
/// than a comparison between two orderings that happen to agree today.
#[derive(Debug, Default)]
pub(crate) struct Oracle {
    /// Every attempt the run dispatched, in order.
    attempts: BTreeMap<(u64, u64), Record>,
    /// How many attempts were dispatched, which is the count no duplicate moves.
    dispatched: u64,
}

impl Oracle {
    /// Record an attempt as dispatched, and what it became.
    pub(crate) fn dispatch(&mut self, action: u64, attempt: u64, fate: Fate) {
        self.attempts
            .insert((action, attempt), Record { attempt, fate });
        self.dispatched = self.dispatched.saturating_add(1);
    }

    /// The fate the fold should report for `action`'s latest attempt, if the oracle
    /// holds one.
    pub(crate) fn latest_of(&self, action: u64) -> Option<Record> {
        self.attempts
            .iter()
            .filter(|entry| entry.0.0 == action)
            .map(|(_, record)| *record)
            .max_by_key(|record| record.attempt)
    }

    /// Record that evidence settled an attempt the oracle had left unknown.
    ///
    /// The oracle is a record of *what is known now*, not of what was once
    /// dispatched, because that is what the fold reports: an attempt reconciled
    /// three continuations ago is not uncertain any more, and an oracle that kept
    /// calling it unknown would be checking against a stale question.
    pub(crate) fn settle(&mut self, action: u64, attempt: u64) {
        if let Some(record) = self.attempts.get_mut(&(action, attempt))
            && record.fate == Fate::Unknown
        {
            record.fate = Fate::Applied;
        }
    }

    /// How many attempts the oracle still holds as unknown.
    pub(crate) fn unknown(&self) -> usize {
        self.attempts
            .values()
            .filter(|record| record.fate == Fate::Unknown)
            .count()
    }
}

/// Picks each attempt's fate from a seeded schedule, without a modulo.
///
/// A modulus is the shorter spelling and it is refused: `arithmetic_side_effects`
/// is forbidden in this workspace, and a countdown is also the thing a seed can be
/// *read* through — the schedule is four counters rather than four divisors, so a
/// seed's fates are visible in the trace rather than buried in arithmetic.
#[derive(Debug)]
struct Schedule {
    /// Attempts until the next unknown one.
    until_unknown: u64,
    /// Attempts until the next verification failure.
    until_failed: u64,
    /// Attempts until the next not-applied one.
    until_not_applied: u64,
    /// How many of each the schedule uses.
    unknown_every: u64,
    failed_every: u64,
    not_applied_every: u64,
}

impl Schedule {
    /// A schedule over the four fates, at the widths the seed chose.
    fn new(sim: &mut sim::Sim) -> Self {
        Self {
            until_unknown: 1,
            until_failed: 1,
            until_not_applied: 1,
            unknown_every: 7 + u64::from(sim.rng().below(5)),
            failed_every: 5,
            not_applied_every: 11,
        }
    }

    /// The fate for the next attempt, and the counters that produce the next one.
    fn next(&mut self) -> Fate {
        if self.until_unknown == 0 {
            self.until_unknown = self.unknown_every;
            Fate::Unknown
        } else {
            self.until_unknown = self.until_unknown.saturating_sub(1);
            if self.until_failed == 0 {
                self.until_failed = self.failed_every;
                Fate::VerificationFailed
            } else {
                self.until_failed = self.until_failed.saturating_sub(1);
                if self.until_not_applied == 0 {
                    self.until_not_applied = self.not_applied_every;
                    Fate::NotApplied
                } else {
                    self.until_not_applied = self.until_not_applied.saturating_sub(1);
                    Fate::Applied
                }
            }
        }
    }
}

/// What a fate folds to, which is the one place that mapping is written down.
pub(crate) const fn status_of(fate: Fate) -> AttemptStatus {
    match fate {
        Fate::Unknown => AttemptStatus::OutcomeUnknown,
        Fate::Applied => AttemptStatus::Verified,
        Fate::NotApplied => AttemptStatus::NotApplied,
        Fate::VerificationFailed => AttemptStatus::VerificationFailed,
    }
}

/// Attempts of one action waiting for their shared flush.
///
/// Every family writes through the shipped group-commit door, [`ATTEMPTS_PER_BATCH`]
/// attempts per `sync_all`, as the long run does and as a controller with a backlog
/// writes them. One flush per attempt made these families a measurement of the
/// device's flush rate rather than of the lifecycle: under a full suite their
/// serialized `F_FULLFSYNC`s took a minute and a half each.
pub(crate) struct Pending {
    /// The action every queued attempt belongs to.
    action: u64,
    /// Queued attempts and the fate each is walked to.
    held: Vec<(u64, Fate)>,
    /// How many are queued, in the batch size's own type.
    queued: u64,
}

impl Pending {
    /// An empty queue for `action`.
    pub(crate) const fn new(action: u64) -> Self {
        Self {
            action,
            held: Vec::new(),
            queued: 0,
        }
    }

    /// Queue one attempt, and flush the batch when it is full or `last` is set.
    ///
    /// Answers whether a flush happened, because only then can the watermark have
    /// moved and a continuation be due.
    pub(crate) fn push(
        &mut self,
        journal: &mut FileJournal,
        oracle: &mut Oracle,
        number: u64,
        fate: Fate,
        last: bool,
    ) -> Result<bool, Box<dyn Error>> {
        self.held.push((number, fate));
        self.queued = self.queued.saturating_add(1);
        if self.queued < ATTEMPTS_PER_BATCH && !last {
            return Ok(false);
        }
        let mut events = Vec::new();
        for &(queued, walked) in &self.held {
            events.extend(ladder(walked, self.action, queued)?);
        }
        journal.compare_and_append_all(&events)?;
        for &(queued, walked) in &self.held {
            oracle.dispatch(self.action, queued, walked);
        }
        self.held.clear();
        self.queued = 0;
        Ok(true)
    }
}

/// Continue `journal` when its own watermark says so, and report whether it did.
///
/// The trigger is the shipped one asked on the shipped path: this is what an
/// unattended controller does, not a test-only schedule.
pub(crate) fn continue_if_due(
    journal: &mut FileJournal,
) -> Result<Option<FileJournal>, Box<dyn Error>> {
    if !journal.continuation_watermark()?.is_due() {
        return Ok(None);
    }
    Ok(Some(journal.continue_as_file()?.ok_or(
        "a continuing journal must hand back a successor",
    )?))
}

/// Assert that a journal's fold still agrees with the oracle.
///
/// The fold is compacted to one record per action, so this cannot compare counts.
/// What it compares is the *latest* attempt of every action the fold holds, whose
/// status must be the oracle's, and the uncertain list, which must hold exactly the
/// attempts the oracle left unknown. That is the whole contract: settled history is
/// sealed and refused, and what is still uncertain is still uncertain.
pub(crate) fn assert_fold_agrees(
    journal: &FileJournal,
    oracle: &Oracle,
    tenant: u64,
) -> TestResult {
    let recovered = journal.recover();
    let mut folded: BTreeMap<u64, (u64, AttemptStatus)> = BTreeMap::new();
    for held in recovered.attempts() {
        let number = action_number(held.key().action())?;
        let attempt = held.key().attempt().get();
        let entry = folded.entry(number).or_insert((attempt, held.status()));
        if attempt >= entry.0 {
            *entry = (attempt, held.status());
        }
    }
    for (action, entry) in &folded {
        let (_, status) = *entry;
        let Some(record) = oracle.latest_of(*action) else {
            continue;
        };
        assert_eq!(
            status,
            status_of(record.fate),
            "tenant {tenant}: the fold reports attempt {} of action {action} as \
             {status:?} where the oracle recorded {:?}",
            record.attempt,
            record.fate
        );
    }
    let unknown = oracle.unknown();
    let carried = recovered
        .attempts()
        .iter()
        .filter(|held| held.status().is_uncertain())
        .count();
    assert_eq!(
        carried, unknown,
        "tenant {tenant}: the fold is uncertain about {carried} attempts where the \
         oracle left {unknown} unknown"
    );
    Ok(())
}

// ── Families ──────────────────────────────────────────────────────────────

/// Every attempt left unknown crosses the boundary and settles there.
fn unresolved_carry(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("carry")?;
        let path = dir.join("run.jrnl");
        let total = 90 + u64::from(sim.rng().below(60));
        let mut stranded: Vec<(u64, EffectKey)> = Vec::new();
        let mut oracle = Oracle::default();
        let mut until_unknown = 9 + u64::from(sim.rng().below(7));
        let mut journal = FileJournal::open_continuing_with(&path, policy()?)?;
        let mut pending = Pending::new(1);

        for number in 1..=total {
            let fate = if until_unknown == 0 {
                until_unknown = 9;
                Fate::Unknown
            } else {
                until_unknown = until_unknown.saturating_sub(1);
                Fate::Applied
            };
            let before = journal.generation();
            let flushed = pending.push(&mut journal, &mut oracle, number, fate, number == total)?;
            if fate == Fate::Unknown {
                stranded.push((number, key_of(1, number)?));
            }
            if !flushed {
                continue;
            }
            if let Some(successor) = continue_if_due(&mut journal)? {
                assert!(
                    !stranded.is_empty(),
                    "a continuation with nothing unresolved to carry is the easy case"
                );
                journal = successor;
                let after = journal.generation();
                // Checked at the boundary rather than at every attempt: the
                // property is that the *carried* set survived, and the attempt that
                // was unknown before the continuation is the one that has to be
                // checked after it.
                for carried in &stranded {
                    let key = carried.1;
                    assert_eq!(
                        journal.recover().status(key),
                        Some(AttemptStatus::OutcomeUnknown),
                        "an attempt left unknown in generation {before} is still \
                         unknown in generation {after}, which is what stops a resend"
                    );
                }
            }
        }
        assert!(
            stranded.len() <= MAX_CHECKPOINT_UNRESOLVED,
            "a run holding {} unresolved attempts is past the declared carry bound \
             of {MAX_CHECKPOINT_UNRESOLVED}, and such a continuation is refused \
             rather than truncating the carry",
            stranded.len()
        );

        // A resend is refused for every one of them, and evidence settles the first
        // few with no second dispatch anywhere.
        let mut settled = 0_u64;
        for &(number, key) in &stranded {
            let resend =
                journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key });
            assert!(
                resend.is_err(),
                "a carried attempt must never be re-admitted: {resend:?}"
            );
            if settled < 3 {
                journal.compare_and_append(
                    journal.tail(),
                    &EffectEvent::OutcomeObserved {
                        key,
                        evidence: EffectEvidence::Applied,
                    },
                )?;
                assert_eq!(
                    journal.recover().status(key),
                    Some(AttemptStatus::Applied),
                    "evidence appended to the successor settles the carried attempt \
                     with no resend and no new attempt"
                );
                oracle.settle(1, number);
                settled = settled.saturating_add(1);
            }
        }
        assert_fold_agrees(&journal, &oracle, 1)?;
        sim.record("unresolved-carried-and-settled");
        sim.trace.record_number("carry-attempts", total);
        sim.trace.record_number("carry-stranded", stranded.len());
        sim.trace
            .record_number("carry-still-unknown", oracle.unknown());
        sim.trace.record_number("carry-settled", settled);
        Ok(())
    })
}

/// Write `predecessors` applied attempts to `path`, continue it explicitly, and
/// answer the byte size of the live successor the generation walk finds.
fn weigh_successor(path: &std::path::Path, predecessors: u64) -> Result<u64, Box<dyn Error>> {
    let mut journal = FileJournal::open_continuing_with(path, policy()?)?;
    let mut oracle = Oracle::default();
    let mut written = 0_u64;
    let mut batch: Vec<EffectEvent> = Vec::new();
    while written < predecessors {
        batch.clear();
        for _ in 0..ATTEMPTS_PER_BATCH.min(predecessors.saturating_sub(written)) {
            written = written.saturating_add(1);
            batch.extend(ladder(Fate::Applied, 1, written)?);
            oracle.dispatch(1, written, Fate::Applied);
        }
        journal.compare_and_append_all(&batch)?;
        if let Some(successor) = continue_if_due(&mut journal)? {
            journal = successor;
        }
    }
    assert_eq!(
        oracle.dispatched, predecessors,
        "every attempt reached the disk for {path:?}"
    );
    let live = journal
        .continue_as_file()?
        .ok_or("a continuing journal must hand back a successor")?
        .path()
        .to_path_buf();
    drop(journal);
    let handle = FileJournal::open_active(path)?;
    assert_eq!(
        handle.path(),
        live.as_path(),
        "the walk finds the live successor for {}",
        path.display()
    );
    assert_fold_agrees(&handle, &oracle, 1)?;
    Ok(std::fs::metadata(live)?.len())
}

/// A successor's own size does not grow with the history behind it.
///
/// Measured on the file: the live successors after a short and a long
/// predecessor are opened through the generation walk and weighed.
fn bounded_reopen(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("reopen")?;
        let jitter = u64::from(sim.rng().below(4));

        // Both journals are continued *explicitly* at the end, so what is weighed is
        // two successors rather than one successor and one file that never reached
        // its trigger. The comparison is then exact rather than a ratio: a
        // successor carries one folded record per action and nothing else, so the
        // one behind two hundred and forty attempts weighs what the one behind two
        // does.
        let ((small_bytes, small_attempts), (long_bytes, long_attempts)) = (
            (
                weigh_successor(&dir.join("short.jrnl"), 2)?,
                2_u64.saturating_add(jitter),
            ),
            (
                weigh_successor(&dir.join("long.jrnl"), 240)?,
                240_u64.saturating_add(jitter),
            ),
        );
        assert_eq!(
            small_bytes, long_bytes,
            "the live successor after {long_attempts} attempts weighs {long_bytes} bytes where \
             one after {small_attempts} weighs {small_bytes}: a successor's cost is its carry, \
             not the history behind it"
        );
        sim.record("reopen-is-flat-in-the-history-behind-it");
        sim.trace
            .record_number("reopen-small-attempts", small_attempts);
        sim.trace.record_number("reopen-small-bytes", small_bytes);
        sim.trace
            .record_number("reopen-long-attempts", long_attempts);
        sim.trace.record_number("reopen-long-bytes", long_bytes);
        Ok(())
    })
}

/// The fold after a *reopen* is exactly what the oracle recorded.
///
/// The other families check the fold on a live handle; this one checks it after a
/// restart, which is the only vantage point that says what a recovered controller
/// would learn rather than what a handle remembers.
fn oracle_matches(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("oracle")?;
        let path = dir.join("run.jrnl");
        let total = 120 + u64::from(sim.rng().below(60));
        let mut oracle = Oracle::default();
        let mut journal = FileJournal::open_continuing_with(&path, policy()?)?;
        let mut schedule = Schedule::new(sim);
        let mut pending = Pending::new(1);
        for number in 1..=total {
            let fate = schedule.next();
            if !pending.push(&mut journal, &mut oracle, number, fate, number == total)? {
                continue;
            }
            if let Some(successor) = continue_if_due(&mut journal)? {
                journal = successor;
                assert_fold_agrees(&journal, &oracle, 1)?;
            }
        }
        let generation = journal.generation();
        drop(journal);

        let reopened = FileJournal::open_active(&path)?;
        assert_eq!(
            reopened.generation(),
            generation,
            "a restart finds the generation the run ended on"
        );
        assert_fold_agrees(&reopened, &oracle, 1)?;
        assert_eq!(
            reopened.recover().uncertain().len(),
            oracle.unknown(),
            "the reopened journal is uncertain about exactly the attempts the oracle \
             left unknown, no more and no fewer"
        );
        sim.record("reopened-fold-matches-the-oracle");
        sim.trace.record_number("oracle-attempts", total);
        sim.trace.record_number("oracle-unknown", oracle.unknown());
        Ok(())
    })
}

// ── The sweep ─────────────────────────────────────────────────────────────

use band_family::band_family;
use tenants::tenant_continuation;

band_family!(
    unresolved_carry_r00 => unresolved_carry, 0; unresolved_carry_r01 => unresolved_carry, 1;

    tenant_continuation_r00 => tenant_continuation, 0;
    tenant_continuation_r01 => tenant_continuation, 1;

    bounded_reopen_r00 => bounded_reopen, 0; bounded_reopen_r01 => bounded_reopen, 1;

    oracle_matches_r00 => oracle_matches, 0; oracle_matches_r01 => oracle_matches, 1;
);

/// Every family owns a distinct scratch tag.
///
/// Two families sharing a tag would race: tests run in parallel processes.
#[test]
fn every_family_has_its_own_scratch_tag() {
    let tags = ["long", "carry", "tenants", "reopen", "oracle"];
    let mut seen = std::collections::BTreeSet::new();
    for tag in tags {
        assert!(seen.insert(tag), "scratch tag {tag} is used twice");
    }
    assert_eq!(seen.len(), tags.len());
}

/// Every fate has a status and every status has a fate, spelled out once.
///
/// The mapping makes the oracle an independent claim rather than a restatement
/// of the journal's fold: a disagreement between the two is a failing test.
#[test]
fn every_fate_names_the_status_a_fold_reports_for_it() {
    assert_eq!(status_of(Fate::Unknown), AttemptStatus::OutcomeUnknown);
    assert_eq!(status_of(Fate::Applied), AttemptStatus::Verified);
    assert_eq!(status_of(Fate::NotApplied), AttemptStatus::NotApplied);
    assert_eq!(
        status_of(Fate::VerificationFailed),
        AttemptStatus::VerificationFailed
    );
    assert!(
        AttemptStatus::OutcomeUnknown.is_uncertain(),
        "and the unknown arm is the one a recovery path must not act on"
    );
}

mod tenants;

mod sim_long_run;
