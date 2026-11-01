//! A store of the caller's own behind durable steps (#781).
//!
//! `HostBuilder::records` installs any [`RunRecords`] through the one door the
//! shipped file store goes through, so every durable step, resume, tenant check
//! and drift check reaches it on the real run path. These rows hold both stores
//! to the published conformance suite and pin the one refusal an ephemeral store
//! earns: an action whose effect leaves the process is refused before it runs,
//! because a crash would leave an empty run and a resume would send the effect
//! blind.
//!
//! The crash rows of `durable_crash_group_commit` and `durable_crash_observation`
//! kill a real child over the file store installed with `run_store`, which now
//! installs it through the same `records` door; they are the `SIGKILL` half of
//! this contract.
//!
//! | Test | What it pins |
//! |---|---|
//! | `the_file_store_conforms` | the shipped store passes every conformance check, reopened from its file |
//! | `an_in_memory_store_conforms` | a forty-line map of the caller's own passes the same checks through the same door |
//! | `an_ephemeral_store_refuses_an_external_action` | an action over a store that cannot survive its writer is refused as `PromiseUnmet` and never runs |
//! | `a_durable_store_admits_the_same_action` | the same action over the file store installed through `records` runs once |

#![cfg(feature = "testing")]

use std::error::Error;
use std::sync::Arc;

use lgwks_bot::GrantSet;
use lgwks_bot::journal::DurabilityPromise;
use lgwks_bot::script::run_store::conformance::{self, Conformance, Opened};
use lgwks_bot::script::{FlowError, RunRecords, Scope};
use lgwks_bot::task::{Host, RunStore, StoreError, task};

use crate::memory_records::MemoryRecords;
use crate::scratch::Scratch;
use crate::script_domains::{SCRIPT_DOMAINS, post, posts_once};
use crate::spec_materialize::take_trace;

type TestResult = Result<(), Box<dyn Error>>;

/// What opening a store under test can fail with.
type OpenError = Box<dyn Error + Send + Sync>;

/// Every check passed, or every violation, named.
fn conformed(store: &str, found: &Conformance) -> TestResult {
    let violations: Vec<String> = found
        .violations()
        .map(|(check, violation)| format!("{check}: {violation}"))
        .collect();
    assert!(
        violations.is_empty(),
        "{store} violated the suite: {violations:#?}"
    );
    assert_eq!(
        found.passed().count(),
        5,
        "{store}: every check ran and passed"
    );
    Ok(())
}

#[test]
fn the_file_store_conforms() -> TestResult {
    let scratch = Scratch::new("records-file")?;
    let path = scratch.path().join("conformance.runstore");
    let mut open = || -> Result<Opened, OpenError> {
        let store = RunStore::open(&path)?;
        let faults = store.clone();
        Ok(Opened::new(Arc::new(store), move || {
            faults.fail_next_index_read();
        }))
    };
    conformed("the file store", &conformance::check(&mut open))
}

#[test]
fn an_in_memory_store_conforms() -> TestResult {
    let memory = MemoryRecords::default();
    let mut open = || -> Result<Opened, OpenError> {
        let faults = memory.clone();
        Ok(Opened::new(Arc::new(memory.clone()), move || {
            faults.fail_next_read();
        }))
    };
    conformed("the in-memory store", &conformance::check(&mut open))
}

/// A host for `acme` resolving the shared script domains over `records`.
fn host_over(records: Arc<dyn RunRecords>) -> Result<Host, Box<dyn Error>> {
    Ok(Host::builder("acme")?
        .domains(&SCRIPT_DOMAINS)
        .grants(GrantSet::empty())
        .records(records)
        .build()?)
}

#[test]
fn an_ephemeral_store_refuses_an_external_action() -> TestResult {
    let host = host_over(Arc::new(MemoryRecords::default()))?;
    let posting = task("posting", |scope: Scope, value: u16| async move {
        post(&scope, "channel", value).await
    })?;
    let _ = take_trace();
    let report = lgwks_bot::block_on(host.run(&posting, 7));
    let refused = report.error().and_then(|error| match *error {
        FlowError::Store { ref source, .. } => Some(&**source),
        _ => None,
    });
    assert!(
        matches!(
            refused,
            Some(&StoreError::PromiseUnmet {
                required: DurabilityPromise::ProcessCrash,
                offered: DurabilityPromise::Ephemeral,
            })
        ),
        "an action over an ephemeral store must be refused as PromiseUnmet, got {:?}",
        report.error()
    );
    assert!(
        take_trace().is_empty(),
        "the refused action must never have run"
    );
    Ok(())
}

#[test]
fn a_durable_store_admits_the_same_action() -> TestResult {
    let scratch = Scratch::new("records-act")?;
    let store = RunStore::open(scratch.path().join("act.runstore"))?;
    posts_once(&host_over(Arc::new(store))?)
}
