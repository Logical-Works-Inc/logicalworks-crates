//! Seeded simulation of the run-store conformance kit's first-attempt check.
//!
//! Source: the `RunRecords::compatibility` contract (`src/script/run_store.rs`
//! and the durable store's own doc): a run the store holds nothing for is a
//! first attempt and compatible by definition, because refusing it makes the
//! run's first record unreachable. The kit held no check for it, so a provider
//! answering `false` there passed conformance and then refused the first step of
//! every run it was given.
//!
//! The fools: the saboteur store answers `false` for every run it never saw;
//! the neighbours record thousands of runs under three tenants before the check
//! asks; the double-clicker runs the whole kit twice on one seed.
//!
//! | Test | What it pins |
//! |---|---|
//! | `sim_first_attempt_a_store_refusing_a_first_attempt_is_named` | the kit names `first_attempt_is_compatible` for a store that answers `false` |
//! | `sim_first_attempt_holds_under_seeded_neighbours` | over seeded tenants' records the check passes, and one seed replays one trace |
//! | `sim_first_attempt_asks_the_trait_twice_and_writes_nothing` | the check's own run is asked one compatibility and one drift, never a lookup or an append |
//! | `sim_first_attempt_the_shipped_binary_reports_it_for_both_stores` | `store-conformance --json` prints the check passing for the file and the memory store |

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};

use lgwks_bot::effect::RunId;
use lgwks_bot::journal::DurabilityPromise;
use lgwks_bot::script::run_store::conformance::{self, Opened, Violation};
use lgwks_bot::script::{Appended, FlowError, RunRecords, Scope, StepKey, StoredValue, Tenant};
use lgwks_bot::task::{DefinitionIdentity, Drift};
use lgwks_std::hash::Digest;
use lgwks_std::seeded::Seeded;

use crate::memory_records::MemoryRecords;

/// A test's outcome: setup failures propagate instead of panicking.
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// What opening a store under test can fail with.
type OpenError = Box<dyn Error + Send + Sync>;

/// The check under test, as the kit and the binary name it.
const CHECK: &str = "first_attempt_is_compatible";

/// Seeds the neighbour sweep replays.
const SEEDS: [u64; 3] = [
    0x5EED_0000_0000_0001,
    0x5EED_0000_0000_0002,
    0xDEAD_BEEF_CAFE_0001,
];

/// Runs the neighbours record before the kit asks about a fresh one.
const NEIGHBOUR_RUNS: usize = 2_000;

/// The tenants the neighbours record under.
const TENANTS: [&str; 3] = ["acme", "widget", "soylent"];

/// A store that answers every run it never recorded as incompatible: the
/// provider bug the check exists to catch.
struct RefusesFirstAttempts(MemoryRecords);

impl RunRecords for RefusesFirstAttempts {
    fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<StoredValue>, FlowError> {
        self.0.lookup(tenant, run, key)
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
        self.0.append(tenant, run, key, path, definition, bytes)
    }

    fn compatibility(
        &self,
        run: RunId,
        definition: &DefinitionIdentity,
    ) -> Result<bool, FlowError> {
        if self.0.record_count(run)? == 0 {
            return Ok(false);
        }
        self.0.compatibility(run, definition)
    }

    fn drift(&self, run: RunId, definition: &DefinitionIdentity) -> Result<Drift, FlowError> {
        self.0.drift(run, definition)
    }

    fn durability(&self) -> DurabilityPromise {
        self.0.durability()
    }

    fn recorded_owner(&self, run: RunId) -> Result<Option<String>, FlowError> {
        self.0.recorded_owner(run)
    }

    fn recorded_definition(&self, run: RunId) -> Result<Option<DefinitionIdentity>, FlowError> {
        self.0.recorded_definition(run)
    }

    fn record_count(&self, run: RunId) -> Result<usize, FlowError> {
        self.0.record_count(run)
    }
}

/// Every trait question a store was asked, by run, in order.
#[derive(Clone, Default)]
struct Asked(Arc<Mutex<BTreeMap<String, Vec<&'static str>>>>);

impl Asked {
    /// The question log, or a refusal naming the thread that poisoned it.
    fn log(&self) -> Result<MutexGuard<'_, BTreeMap<String, Vec<&'static str>>>, FlowError> {
        self.0.lock().map_err(|poisoned| {
            FlowError::failed(format!("the question log is poisoned: {poisoned}"))
        })
    }

    /// Records that `run` was asked `question`.
    fn note(&self, run: RunId, question: &'static str) -> Result<(), FlowError> {
        self.log()?
            .entry(format!("{run:?}"))
            .or_default()
            .push(question);
        Ok(())
    }

    /// The question lists of every run that was never read or written.
    fn untouched(&self) -> Result<Vec<Vec<&'static str>>, FlowError> {
        Ok(self
            .log()?
            .values()
            .filter(|questions| {
                !questions
                    .iter()
                    .any(|question| matches!(*question, "lookup" | "append"))
            })
            .cloned()
            .collect())
    }
}

/// The memory store, with every question it is asked noted by run.
struct Counting {
    /// The store answering.
    inner: MemoryRecords,
    /// What it was asked.
    asked: Asked,
}

impl RunRecords for Counting {
    fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<StoredValue>, FlowError> {
        self.asked.note(run, "lookup")?;
        self.inner.lookup(tenant, run, key)
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
        self.asked.note(run, "append")?;
        self.inner.append(tenant, run, key, path, definition, bytes)
    }

    fn compatibility(
        &self,
        run: RunId,
        definition: &DefinitionIdentity,
    ) -> Result<bool, FlowError> {
        self.asked.note(run, "compatibility")?;
        self.inner.compatibility(run, definition)
    }

    fn drift(&self, run: RunId, definition: &DefinitionIdentity) -> Result<Drift, FlowError> {
        self.asked.note(run, "drift")?;
        self.inner.drift(run, definition)
    }

    fn durability(&self) -> DurabilityPromise {
        self.inner.durability()
    }

    fn recorded_owner(&self, run: RunId) -> Result<Option<String>, FlowError> {
        self.asked.note(run, "recorded_owner")?;
        self.inner.recorded_owner(run)
    }

    fn recorded_definition(&self, run: RunId) -> Result<Option<DefinitionIdentity>, FlowError> {
        self.asked.note(run, "recorded_definition")?;
        self.inner.recorded_definition(run)
    }

    fn record_count(&self, run: RunId) -> Result<usize, FlowError> {
        self.asked.note(run, "record_count")?;
        self.inner.record_count(run)
    }
}

/// The kit run over `records`, every handle onto the same store.
fn kit_over(records: &Arc<dyn RunRecords>, memory: &MemoryRecords) -> conformance::Conformance {
    let mut open = || -> Result<Opened, OpenError> {
        let faults = memory.clone();
        Ok(Opened::new(Arc::clone(records), move || {
            faults.fail_next_read();
        }))
    };
    conformance::check(&mut open)
}

/// The saboteur answers `false` for a run it never saw, and the kit names the
/// first-attempt check, as an answer the contract forbids, rather than passing it.
#[test]
fn sim_first_attempt_a_store_refusing_a_first_attempt_is_named() {
    let memory = MemoryRecords::default();
    let records: Arc<dyn RunRecords> = Arc::new(RefusesFirstAttempts(memory.clone()));
    let found = kit_over(&records, &memory);
    let named: Vec<&Violation> = found
        .violations()
        .filter(|entry| entry.0 == CHECK)
        .map(|entry| entry.1)
        .collect();
    assert!(
        matches!(named.as_slice(), [Violation::Answered { .. }]),
        "the saboteur's first attempt is named: {named:?}"
    );
    assert!(
        !found.passed().any(|check| check == CHECK),
        "a store refusing a first attempt never passes it"
    );
}

/// The step key the neighbours record under, for `tenant`.
fn neighbour_key(tenant: &str) -> TestResult<StepKey> {
    Ok(Scope::root(Tenant::new(tenant)?).enter("neighbour")?.key())
}

/// One seed's trace: who recorded what, then which checks passed over that store.
fn trace_of(seed: u64) -> TestResult<String> {
    let memory = MemoryRecords::default();
    let mut rng = Seeded::from_seed(seed);
    let definition = DefinitionIdentity::new("neighbour", 1, Digest::from_bytes([5; 32]), 1);
    let mut recorded = [0_usize; TENANTS.len()];
    for index in 0..NEIGHBOUR_RUNS {
        let pick = usize::try_from(rng.below(3)?)?;
        let (Some(tenant), Some(count)) = (TENANTS.get(pick), recorded.get_mut(pick)) else {
            let refusal = format!("seed {seed} picked tenant {pick} of {}", TENANTS.len());
            lgwks_std::trace::debug!(error = %refusal, "trace_of: the seed left the tenant table");
            return Err(refusal.into());
        };
        let run = RunId::mint()?;
        let answer = memory.append(
            tenant,
            run,
            neighbour_key(tenant)?,
            "neighbour",
            &definition,
            index.to_le_bytes().to_vec(),
        )?;
        if answer != Appended::Recorded {
            let refusal = format!("seed {seed}: neighbour {index} answered {answer:?}");
            lgwks_std::trace::debug!(error = %refusal, "trace_of: a neighbour was not recorded");
            return Err(refusal.into());
        }
        *count = count.saturating_add(1);
    }
    let records: Arc<dyn RunRecords> = Arc::new(memory.clone());
    let found = kit_over(&records, &memory);
    let passed: Vec<&str> = found.passed().collect();
    assert!(
        passed.contains(&CHECK),
        "seed {seed}: a fresh run is a first attempt among {NEIGHBOUR_RUNS} neighbours: {passed:?}"
    );
    Ok(format!("{recorded:?} {passed:?}"))
}

/// Thousands of neighbour runs under three tenants never make a fresh run look
/// recorded, and the same seed replays the same trace.
#[test]
fn sim_first_attempt_holds_under_seeded_neighbours() -> TestResult {
    for seed in SEEDS {
        let first = trace_of(seed)?;
        assert_eq!(first, trace_of(seed)?, "seed {seed} replays");
    }
    Ok(())
}

/// The check asks the trait directly: its own run is asked one compatibility
/// and one drift, and nothing reads or writes it, unlike the host-driven checks
/// that run a whole task per question.
#[test]
fn sim_first_attempt_asks_the_trait_twice_and_writes_nothing() -> TestResult {
    let memory = MemoryRecords::default();
    let asked = Asked::default();
    let records: Arc<dyn RunRecords> = Arc::new(Counting {
        inner: memory.clone(),
        asked: asked.clone(),
    });
    let found = kit_over(&records, &memory);
    assert!(
        found.passed().any(|check| check == CHECK),
        "the check ran and passed"
    );
    assert_eq!(
        asked.untouched()?,
        vec![vec!["compatibility", "drift"]],
        "exactly one run is never read or written, and it is asked exactly twice"
    );
    Ok(())
}

/// The binary under test, wherever this run placed it.
fn binary() -> PathBuf {
    for name in [
        "NEXTEST_BIN_EXE_store-conformance",
        "NEXTEST_BIN_EXE_store_conformance",
    ] {
        if let Some(found) = std::env::var_os(name) {
            return PathBuf::from(found);
        }
    }
    PathBuf::from(env!("CARGO_BIN_EXE_store-conformance"))
}

/// The shipped binary reports the check for both stores it runs, as an agent
/// reading `--json` sees it, and the run still exits 0.
#[test]
fn sim_first_attempt_the_shipped_binary_reports_it_for_both_stores() -> TestResult {
    let output = Command::new(binary()).arg("--json").output()?;
    let stdout = String::from_utf8(output.stdout)?;
    for store in ["file", "memory"] {
        let line = format!(r#"{{"store":"{store}","check":"{CHECK}","pass":true}}"#);
        assert!(
            stdout.lines().any(|seen| seen == line),
            "{store}: {line} in {stdout}"
        );
    }
    assert!(
        output.status.success(),
        "a conforming run exits 0: {stdout}"
    );
    Ok(())
}
