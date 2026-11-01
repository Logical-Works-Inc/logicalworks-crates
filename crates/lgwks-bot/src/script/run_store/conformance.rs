//! The conformance suite any [`RunRecords`] implementation runs.
//!
//! A store of the caller's own is installed with
//! [`HostBuilder::records`](crate::task::HostBuilder::records) and is reached by
//! every durable step through the door the shipped file store is reached through.
//! This suite is what makes "the crash tests that pass for the built-in store pass
//! for mine" a check rather than a hope: each check drives the real [`Host`] and
//! [`remember`] front door against the store under test, or the trait itself
//! where the property is the trait's own.
//!
//! | Check | What it pins |
//! |---|---|
//! | `append_is_durable` | a run recorded through one handle replays through a fresh handle over the same storage, and its step body never runs again |
//! | `read_failure_is_not_absence` | a failed read reaches the step as an error, never as a miss that re-runs a finished step (INV-BOT-7), and the store is not left failing |
//! | `tenants_are_isolated` | a second tenant can neither resume nor read the first tenant's run, and its step body never runs |
//! | `refused_append_changes_nothing` | an identical append is idempotent, and a conflicting or foreign-tenant append leaves the committed record and the count as they were, across a reopen |
//! | `drift_is_reported` | a resume under another definition is refused as a typed drift before any body runs |
//!
//! [`check`] runs every check every time, whatever an earlier one found, and the
//! [`Conformance`] it returns names each one that passed and each [`Violation`].
//! The `store-conformance` binary runs it against the shipped file store and an
//! in-memory store of the caller's own.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lgwks_std::hash::Digest;

use crate::effect::RunId;
use crate::task::{DefinitionIdentity, Disposition, Host, Report, Task, task};

use super::super::{FlowError, Scope, StepKey, Tenant, remember};
use super::{Appended, RunRecords};

/// The tenant every check records under.
const OWNER: &str = "conformance-owner";

/// A second tenant over the same storage, which must see nothing of [`OWNER`]'s.
const STRANGER: &str = "conformance-stranger";

/// The value every run is given and must return.
const INPUT: u32 = 7;

/// The step every run's body remembers.
const STEP: &str = "value";

/// What opening the store under test can fail with.
type OpenError = Box<dyn Error + Send + Sync>;

/// One handle onto the store under test, and the fault the suite arms on it.
///
/// Every call of the suite's `open` must return a fresh handle over the *same*
/// storage, as a restarted process would open it: that is how the suite tells a
/// durable append from a map that is still warm.
pub struct Opened {
    /// The handle a host installs.
    records: Arc<dyn RunRecords>,
    /// Arm a one-shot failure of the next read a durable step makes through this
    /// handle: [`RunRecords::lookup`] or [`RunRecords::compatibility`].
    fail_next_read: Box<dyn Fn() + Send + Sync>,
}

impl Opened {
    /// A handle and the injector that makes its next step read fail once.
    ///
    /// The injector must fail exactly one read and leave the store readable after
    /// it, so the suite can tell an error from a store that broke for good.
    #[must_use]
    pub fn new(
        records: Arc<dyn RunRecords>,
        fail_next_read: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            records,
            fail_next_read: Box::new(fail_next_read),
        }
    }
}

/// What a check found the store, or the host over it, doing wrong.
///
/// Each arm names the moment of the check it was found in, so a report reads as
/// "during the resume through a fresh handle, the body ran twice" rather than as
/// a bare count.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// The store, or a host over it, refused what the check needed to set up.
    Setup {
        /// The moment of the check.
        during: &'static str,
        /// What refused, as it rendered.
        cause: String,
    },
    /// A step body ran `times` times where the contract allows exactly one.
    Reran {
        /// The moment of the check.
        during: &'static str,
        /// How often the body has run.
        times: u64,
    },
    /// A run ended other than the contract requires.
    Ended {
        /// The moment of the check.
        during: &'static str,
        /// The disposition, output and error it ended with.
        found: String,
    },
    /// A direct call on the store answered something the contract forbids.
    Answered {
        /// The moment of the check.
        during: &'static str,
        /// The answer, as it rendered.
        found: String,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Setup { during, ref cause } => write!(f, "{during}: setup refused: {cause}"),
            Self::Reran { during, times } => write!(
                f,
                "{during}: the step body has run {times} times where the contract allows once"
            ),
            Self::Ended { during, ref found } => write!(f, "{during}: the run ended {found}"),
            Self::Answered { during, ref found } => {
                write!(f, "{during}: the store answered {found}")
            }
        }
    }
}

impl Error for Violation {}

/// What [`check`] found: each check that passed and each violation, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conformance {
    /// Every check, in the order it ran, with the violation it found if any.
    outcomes: Vec<(&'static str, Result<(), Violation>)>,
}

impl Conformance {
    /// The checks that passed, in the order they ran.
    pub fn passed(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.outcomes
            .iter()
            .filter(|entry| entry.1.is_ok())
            .map(|entry| entry.0)
    }

    /// Each check that failed and what it found, in the order they ran.
    pub fn violations(&self) -> impl Iterator<Item = (&'static str, &Violation)> + '_ {
        self.outcomes.iter().filter_map(|entry| match entry.1 {
            Ok(()) => None,
            Err(ref found) => Some((entry.0, found)),
        })
    }
}

/// Run every check against the store `open` returns.
///
/// `open` is called once per handle a check needs, and must return a fresh handle
/// over the same storage each time. Every check runs, whatever an earlier one
/// found; each records its runs under run ids of its own, so no check reads
/// another's records.
pub fn check(open: &mut dyn FnMut() -> Result<Opened, OpenError>) -> Conformance {
    let checks: [(&'static str, Check); 5] = [
        ("append_is_durable", append_is_durable),
        ("read_failure_is_not_absence", read_failure_is_not_absence),
        ("tenants_are_isolated", tenants_are_isolated),
        (
            "refused_append_changes_nothing",
            refused_append_changes_nothing,
        ),
        ("drift_is_reported", drift_is_reported),
    ];
    let mut outcomes = Vec::with_capacity(checks.len());
    for (name, run) in checks {
        let outcome = run(open);
        if let Err(ref found) = outcome {
            lgwks_std::trace::warn!(check = name, found = %found, "run-store conformance: violation");
        }
        outcomes.push((name, outcome));
    }
    Conformance { outcomes }
}

/// `found`, logged where the check found it.
fn violated<T>(found: Violation) -> Result<T, Violation> {
    lgwks_std::trace::debug!(found = %found, "run-store conformance: a check found a violation");
    Err(found)
}

/// The setup refusal `cause`, during `during`.
fn setup(during: &'static str) -> impl FnOnce(&dyn fmt::Display) -> Violation {
    move |cause| Violation::Setup {
        during,
        cause: cause.to_string(),
    }
}

/// The body ran exactly once by `during`, or the violation it did not.
fn ran_once(bodies: &AtomicU64, during: &'static str) -> Result<(), Violation> {
    match bodies.load(Ordering::SeqCst) {
        1 => Ok(()),
        times => violated(Violation::Reran { during, times }),
    }
}

/// One check: `Err` names what the store did wrong.
type Check = fn(&mut dyn FnMut() -> Result<Opened, OpenError>) -> Result<(), Violation>;

/// The future a counted body returns.
type Body = Pin<Box<dyn Future<Output = Result<u32, FlowError>>>>;

/// A one-step durable task whose step body counts how often it really runs.
fn counted(bodies: &Arc<AtomicU64>) -> Result<Task<impl Fn(Scope, u32) -> Body>, Violation> {
    let bodies = Arc::clone(bodies);
    task("conformance", move |scope: Scope, input: u32| -> Body {
        let bodies = Arc::clone(&bodies);
        Box::pin(async move {
            remember(&scope, STEP, || async move {
                bodies.fetch_add(1, Ordering::SeqCst);
                Ok::<u32, FlowError>(input)
            })
            .await
        })
    })
    .map_err(|refused| setup("the task's definition")(&refused))
}

/// A host for `tenant` over the store `opened` holds.
fn host_over(tenant: &str, opened: &Opened) -> Result<Host, Violation> {
    Host::builder(tenant)
        .map_err(|refused| setup("the host's tenant")(&refused))?
        .records(Arc::clone(&opened.records))
        .build()
        .map_err(|refused| setup("the host's build")(&refused))
}

/// Open a handle for `during`.
fn open_for(
    open: &mut dyn FnMut() -> Result<Opened, OpenError>,
    during: &'static str,
) -> Result<Opened, Violation> {
    open().map_err(|refused| setup(during)(&refused))
}

/// `report` succeeded with [`INPUT`], or how it ended instead.
fn returned_input(report: &Report<u32>, during: &'static str) -> Result<(), Violation> {
    match (report.disposition(), report.output()) {
        (Disposition::Succeeded, Some(&INPUT)) => Ok(()),
        (disposition, output) => violated(Violation::Ended {
            during,
            found: format!(
                "{disposition:?} returning {output:?} ({:?}), not success returning {INPUT}",
                report.error()
            ),
        }),
    }
}

/// `report` was refused, or how it ended instead.
fn refused(report: &Report<u32>, during: &'static str) -> Result<(), Violation> {
    if report.disposition() == Disposition::Succeeded {
        return violated(Violation::Ended {
            during,
            found: format!("Succeeded returning {:?}, not refused", report.output()),
        });
    }
    Ok(())
}

/// The run id `report` names.
fn run_of(report: &Report<u32>, during: &'static str) -> Result<RunId, Violation> {
    match report.run_id() {
        Some(run) => Ok(run),
        None => violated(Violation::Ended {
            during,
            found: String::from("with no run id over an installed store"),
        }),
    }
}

/// Run the counted task once for [`OWNER`] and hand back the run it recorded.
fn recorded(
    open: &mut dyn FnMut() -> Result<Opened, OpenError>,
    bodies: &Arc<AtomicU64>,
) -> Result<RunId, Violation> {
    let opened = open_for(open, "the first run")?;
    let host = host_over(OWNER, &opened)?;
    let report = crate::block_on(host.run(&counted(bodies)?, INPUT));
    returned_input(&report, "the first run")?;
    run_of(&report, "the first run")
}

/// The run recorded through one handle replays through a fresh one.
fn append_is_durable(open: &mut dyn FnMut() -> Result<Opened, OpenError>) -> Result<(), Violation> {
    let bodies = Arc::new(AtomicU64::new(0));
    let run = recorded(open, &bodies)?;
    let reopened = open_for(open, "the resume through a fresh handle")?;
    let host = host_over(OWNER, &reopened)?;
    let report = crate::block_on(host.resume(run, &counted(&bodies)?, INPUT));
    returned_input(&report, "the resume through a fresh handle")?;
    ran_once(&bodies, "the resume through a fresh handle")
}

/// A failed read is an error at the step, never a miss that re-runs it.
fn read_failure_is_not_absence(
    open: &mut dyn FnMut() -> Result<Opened, OpenError>,
) -> Result<(), Violation> {
    let bodies = Arc::new(AtomicU64::new(0));
    let run = recorded(open, &bodies)?;
    let opened = open_for(open, "the faulted resume")?;
    let host = host_over(OWNER, &opened)?;
    let work = counted(&bodies)?;
    (opened.fail_next_read)();
    let faulted = crate::block_on(host.resume(run, &work, INPUT));
    refused(&faulted, "the resume whose step read failed")?;
    ran_once(&bodies, "the resume whose step read failed")?;
    let after = crate::block_on(host.resume(run, &work, INPUT));
    returned_input(&after, "the resume after the one-shot fault")?;
    ran_once(&bodies, "the resume after the one-shot fault")
}

/// A second tenant can neither resume nor read the first tenant's run.
fn tenants_are_isolated(
    open: &mut dyn FnMut() -> Result<Opened, OpenError>,
) -> Result<(), Violation> {
    let bodies = Arc::new(AtomicU64::new(0));
    let run = recorded(open, &bodies)?;
    let opened = open_for(open, "the stranger's resume")?;
    let stranger = host_over(STRANGER, &opened)?;
    let report = crate::block_on(stranger.resume(run, &counted(&bodies)?, INPUT));
    refused(&report, "another tenant's resume of the owner's run")?;
    ran_once(&bodies, "another tenant's resume of the owner's run")?;
    let key = probe_key(STRANGER)?;
    match opened.records.lookup(STRANGER, run, key) {
        Ok(None) => Ok(()),
        Err(refusal) => {
            lgwks_std::trace::debug!(error = %refusal, "run-store conformance: a foreign lookup was refused, as it must be");
            Ok(())
        }
        Ok(Some(_)) => violated(Violation::Answered {
            during: "a lookup under another tenant",
            found: String::from("a record from the owner's run"),
        }),
    }
}

/// `answer` is `expected`, or the violation it is not.
fn answered(
    answer: Result<Appended, FlowError>,
    expected: Appended,
    during: &'static str,
) -> Result<(), Violation> {
    match answer {
        Ok(got) if got == expected => Ok(()),
        other => violated(Violation::Answered {
            during,
            found: format!("{other:?}, not {expected:?}"),
        }),
    }
}

/// An identical append is idempotent; a conflicting or foreign one changes nothing.
fn refused_append_changes_nothing(
    open: &mut dyn FnMut() -> Result<Opened, OpenError>,
) -> Result<(), Violation> {
    let opened = open_for(open, "the appends")?;
    let records = &opened.records;
    let run = RunId::mint().map_err(|refusal| setup("minting a run id")(&refusal))?;
    let key = probe_key(OWNER)?;
    let definition =
        DefinitionIdentity::new("conformance-probe", 1, Digest::from_bytes([7; 32]), 1);
    let committed = vec![1, 2, 3];
    let append = |tenant: &str, bytes: Vec<u8>| {
        records.append(tenant, run, key, "conformance-probe", &definition, bytes)
    };
    answered(
        append(OWNER, committed.clone()),
        Appended::Recorded,
        "a first append",
    )?;
    answered(
        append(OWNER, committed.clone()),
        Appended::AlreadyRecorded,
        "an identical second append",
    )?;
    match append(OWNER, vec![9, 9, 9]) {
        Ok(Appended::Conflicting) => {}
        Err(refusal) => {
            lgwks_std::trace::debug!(error = %refusal, "run-store conformance: a conflicting append was refused outright");
        }
        other => {
            return violated(Violation::Answered {
                during: "a different value under a committed key",
                found: format!("{other:?}, not Conflicting or a refusal"),
            });
        }
    }
    if let Ok(answer) = append(STRANGER, vec![4, 4, 4]) {
        return violated(Violation::Answered {
            during: "another tenant's append to the owner's run",
            found: format!("{answer:?}, not a refusal"),
        });
    }
    unchanged(
        records.as_ref(),
        run,
        key,
        &committed,
        "after the refused appends",
    )?;
    drop(opened);
    let reopened = open_for(open, "the reopen after the appends")?;
    unchanged(
        reopened.records.as_ref(),
        run,
        key,
        &committed,
        "the reopen after the appends",
    )
}

/// The committed record under `key` is `committed`, and the run holds one record.
fn unchanged(
    records: &dyn RunRecords,
    run: RunId,
    key: StepKey,
    committed: &[u8],
    during: &'static str,
) -> Result<(), Violation> {
    let held = match records.lookup(OWNER, run, key) {
        Ok(Some(held)) => held,
        other => {
            return violated(Violation::Answered {
                during,
                found: format!("{other:?} for the committed record"),
            });
        }
    };
    if held.bytes() != committed {
        return violated(Violation::Answered {
            during,
            found: String::from("the committed record with changed bytes"),
        });
    }
    match records.record_count(run) {
        Ok(1) => Ok(()),
        other => violated(Violation::Answered {
            during,
            found: format!("{other:?} for the run's record count, not 1"),
        }),
    }
}

/// A step key no counted run uses, under `tenant`.
fn probe_key(tenant: &str) -> Result<StepKey, Violation> {
    let tenant = Tenant::new(tenant).map_err(|refusal| setup("the probe's tenant")(&refusal))?;
    Scope::root(tenant)
        .enter("conformance-probe")
        .map(|probe| probe.record_key())
        .map_err(|refusal| setup("the probe's step")(&refusal))
}

/// A resume under another definition is refused as a typed drift.
fn drift_is_reported(open: &mut dyn FnMut() -> Result<Opened, OpenError>) -> Result<(), Violation> {
    let bodies = Arc::new(AtomicU64::new(0));
    let opened = open_for(open, "the run under revision 1")?;
    let host = host_over(OWNER, &opened)?;
    let work = counted(&bodies)?;
    let first = host.definition("conformance", 1, None, 1);
    let report = crate::block_on(host.run_under(&first, &work, INPUT));
    returned_input(&report, "the run under revision 1")?;
    let run = run_of(&report, "the run under revision 1")?;
    let revised = host.definition("conformance", 2, None, 1);
    let resumed = crate::block_on(host.resume_under(run, &revised, &work, INPUT));
    if !matches!(resumed.error(), Some(&FlowError::Incompatible { .. })) {
        return violated(Violation::Ended {
            during: "the resume under revision 2",
            found: format!(
                "{:?} with {:?}, not a typed drift",
                resumed.disposition(),
                resumed.error()
            ),
        });
    }
    ran_once(&bodies, "the resume under revision 2")
}
