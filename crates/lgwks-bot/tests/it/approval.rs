//! Human approvals through the public run path (#784).
//!
//! | Requirement | Test |
//! |---|---|
//! | APR-1 | `a_first_run_parks_holding_no_task` |
//! | APR-2 | `a_restarted_host_decides_a_parked_approval` |
//! | APR-3 | `a_decision_against_other_bytes_is_a_mismatch_and_runs_nothing` |
//! | APR-4 | `a_foreign_decision_is_refused_and_audited` |
//! | APR-5 | `a_lapsed_approval_fails_typed_and_decides_expired` |
//! | APR-6 | `a_park_past_the_ceiling_is_refused_before_the_body_runs` |
//! | APR-7 | `an_external_act_without_an_approval_is_refused` |
//! | APR-8 | `an_approved_act_runs_once_on_the_approved_bytes` |
//! | APR-9 | `two_racing_decisions_record_exactly_one` |
//!
//! Every flow below is expanded by the real macro and run by a real `Host`
//! over a real run-store file; the scratch directory is `crate::scratch`, so
//! no run inherits another's records. Each test names the requirement it
//! proves in its table row above.
//!
//! The error type is an enum over the run path's own refusals, so a failure
//! names the fact that moved rather than a string.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lgwks_bot::script::{ApprovalDecision, Clock, DecisionError, FlowError, Scope};
use lgwks_bot::task::{Disposition, Host, Report, RunStore, StoreError, StoreLimitKind, task};
use lgwks_bot::{Action, Auth, BotError, Cap, EffectLifetime, Execute};

use crate::effect_policy::allowing;
use crate::scratch::Scratch;

use crate::approval_error::ApprovalError as ApprovalTestError;
use crate::sim_approval_extra::{BoxedRun, ByteTask, lapse, run_of, scratch};

/// The digest `host` lists for the undecided approval `id`: the bytes a
/// decision must name to be recorded.
fn listed_digest(host: &Host, id: &str) -> Result<lgwks_std::hash::Digest, ApprovalTestError> {
    match host.pending(100)?.iter().find(|entry| entry.id() == id) {
        Some(entry) => Ok(*entry.digest()),
        None => fail("the park is listed"),
    }
}

/// Run `parking` on `payload` until it parks: the id, run and listed digest
/// that a decision and the resume after it name.
fn park_listed<F, Fut>(
    host: &Host,
    parking: &lgwks_bot::task::Task<F>,
    payload: &[u8],
) -> Result<(String, lgwks_bot::effect::RunId, lgwks_std::hash::Digest), ApprovalTestError>
where
    F: Fn(Scope, Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, FlowError>>,
{
    let report = lgwks_bot::block_on(host.run(parking, payload.to_vec()));
    let id = parked_id(&report)?;
    let run = run_of(&report)?;
    let digest = listed_digest(host, &id)?;
    Ok((id, run, digest))
}

/// What a gated-release test runs over: its scratch store, a releasing host
/// for `acme`, and the `guarded` flow as a task.
type ReleaseFixture = (Scratch, Host, ByteTask);

/// One run of `guarded`.
fn guarded_run(scope: Scope, payload: Vec<u8>) -> BoxedRun {
    Box::pin(async move { guarded(&scope, payload).await })
}

/// A fresh gated-release fixture, with this thread's payload notes and the
/// release count cleared so no earlier test's numbers carry over.
fn release_fixture(tag: &str) -> Result<ReleaseFixture, ApprovalTestError> {
    clear_noted();
    RELEASED.store(0, Ordering::SeqCst);
    let scratch = scratch(tag)?;
    let host = release_host_on(scratch.path(), "acme")?;
    let body: fn(Scope, Vec<u8>) -> BoxedRun = guarded_run;
    let guarding = task("guarded", body)?;
    Ok((scratch, host, guarding))
}

/// Log and return the fixture refusal `what`, so the scan sees the signal.
fn fail<T>(what: &'static str) -> Result<T, ApprovalTestError> {
    lgwks_std::trace::debug!(what, "approval fixture: refusing the test");
    Err(ApprovalTestError::Setup { what })
}

/// A host for `tenant` over the run store in `dir`.
fn host_on(dir: &std::path::Path, tenant: &str) -> Result<Host, ApprovalTestError> {
    Ok(Host::builder(tenant)?.run_store(dir)?.build()?)
}

/// A host for `tenant` over the run store in `dir`, measuring time on a
/// virtual clock the test advances.
fn virtual_host_on(dir: &std::path::Path, tenant: &str) -> Result<Host, ApprovalTestError> {
    Ok(Host::builder(tenant)?
        .run_store(dir)?
        .clock(Clock::virtual_at(Duration::ZERO))
        .build()?)
}

// The payloads one run's payload block produced, in order.
//
// Thread-local rather than shared, because the suite runs tests on many
// threads and one global count would let two tests move each other's
// numbers. What the count proves is per payload: a resume replays the
// recorded payload and never re-polls the block.
thread_local! {
    static NOTED: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

/// Record `payload` as produced, and hand it back.
fn note(payload: Vec<u8>) -> Vec<u8> {
    // A plain borrow: nothing on this thread holds the cell across a call, and
    // a note dropped on a conflict would hide a re-poll from every count below.
    NOTED.with(|noted| noted.borrow_mut().push(payload.clone()));
    payload
}

/// Every payload the payload block produced on this thread.
fn noted() -> Vec<Vec<u8>> {
    NOTED.with(|noted| noted.borrow().clone())
}

/// Forget what the payload block produced on this thread.
fn clear_noted() {
    NOTED.with(|noted| noted.borrow_mut().clear());
}

/// How many times the gated action ran on this thread.
static RELEASED: AtomicUsize = AtomicUsize::new(0);

/// An action whose effect leaves the process and archives its input as the
/// payload's wire record, so an approval parked over the same bytes releases it.
struct Release;

impl Release {
    /// Build from any target.
    fn from_target(_target: &str) -> Result<Action, BotError> {
        Ok(Action::new(Self))
    }
}

impl Execute for Release {
    /// The payload, as the approval parked it.
    type Input = Vec<u8>;
    /// It answers nothing.
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::External
    }

    fn archive_input(&self, input: &Vec<u8>) -> Option<Vec<u8>> {
        // Through the `script` re-export: `script::run_store` is public only
        // under `testing`, which the Windows containment job does not enable.
        lgwks_bot::script::Durable::to_record(input).ok()
    }

    async fn execute_action(&self, call: (Auth, &Vec<u8>)) -> Result<(), BotError> {
        call.0.check(&[])?;
        RELEASED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::release"
    }
}

lgwks_bot::domains! {
    /// `test::release` bound to the stub the gated act runs.
    pub(crate) APPROVAL_DOMAINS {
        observe {}
        execute {
            "test::release" => Release::from_target,
        }
    }
}

/// A host for `tenant` over the run store in `dir`, resolving the release
/// action.
fn release_host_on(dir: &std::path::Path, tenant: &str) -> Result<Host, ApprovalTestError> {
    Ok(Host::builder(tenant)?
        .run_store(dir)?
        .domains(&APPROVAL_DOMAINS)
        .build()?)
}

lgwks_bot::script! {
    /// Park the payload for a human decision, then give it back.
    pub(crate) flow ship(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            note(payload)
        give back approved.payload().to_vec()

    /// Park the payload, then run the gated action on the approved bytes.
    pub(crate) flow guarded(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            note(payload)
        act test::release on "dock" with approved.payload().to_vec()
        give back approved.payload().to_vec()

    /// Run the gated action with no approval parked.
    pub(crate) flow direct(payload: Vec<u8>):
        act test::release on "dock" with payload
}

/// The parked id a report carries, or the fixture that says it did not park.
fn parked_id<O>(report: &Report<O>) -> Result<String, ApprovalTestError> {
    let Some(error) = report.error() else {
        return fail("a parked run names its id");
    };
    match *error {
        FlowError::Parked { ref id, .. } => Ok(id.clone()),
        _ => fail("a parked run names its id"),
    }
}

/// APR-1: a first run parks without holding a task.
#[test]
fn a_first_run_parks_holding_no_task() -> Result<(), ApprovalTestError> {
    clear_noted();
    let scratch = scratch("approval-park")?;
    let host = host_on(scratch.path(), "acme")?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let payload = b"release-42".to_vec();
    let report = lgwks_bot::block_on(host.run(&shipping, payload.clone()));
    if !matches!(report.disposition(), Disposition::Parked) {
        return Err(ApprovalTestError::Setup {
            what: "the first run parks",
        });
    }
    let id = parked_id(&report)?;
    let listed = host.pending(100)?;
    if listed.len() != 1 || listed[0].id() != id {
        return Err(ApprovalTestError::Setup {
            what: "the park is listed once under its id",
        });
    }
    if noted() != [payload] {
        return Err(ApprovalTestError::Setup {
            what: "the payload block ran once, then the run parked",
        });
    }
    Ok(())
}

/// APR-2: a restart parks nothing new: the pending record survives the
/// process, a decision on the reopened store records, and the resume returns
/// the payload.
#[test]
fn a_restarted_host_decides_a_parked_approval() -> Result<(), ApprovalTestError> {
    clear_noted();
    let scratch = scratch("approval-restart")?;
    let payload = b"release-43".to_vec();
    // One task for both hosts: a task names the flow, not the host that runs
    // it, so the resume after the restart replays the same definition.
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let (run, id) = {
        let host = host_on(scratch.path(), "acme")?;
        let report = lgwks_bot::block_on(host.run(&shipping, payload.clone()));
        let id = parked_id(&report)?;
        let run = report.run_id().ok_or(ApprovalTestError::Setup {
            what: "a parked run names its run",
        })?;
        (run, id)
    };
    let reopened = host_on(scratch.path(), "acme")?;
    let listed = reopened.pending(100)?;
    if listed.len() != 1 || listed[0].id() != id {
        return Err(ApprovalTestError::Setup {
            what: "the park survives the restart",
        });
    }
    let digest = *listed[0].digest();
    reopened.decide(&id, ApprovalDecision::approve("reopener", digest))?;
    // A decision twice recorded is refused, not merged.
    match reopened.decide(&id, ApprovalDecision::approve("reopener", digest)) {
        Err(DecisionError::AlreadyDecided { .. }) => {}
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "a second decision is AlreadyDecided",
            });
        }
    }
    let resumed = lgwks_bot::block_on(reopened.resume(run, &shipping, payload.clone()));
    if !matches!(resumed.disposition(), Disposition::Succeeded)
        || resumed.output() != Some(&payload)
    {
        return Err(ApprovalTestError::Setup {
            what: "the resume after the decision returns the payload",
        });
    }
    if noted() != [payload] {
        return Err(ApprovalTestError::Setup {
            what: "the payload block ran once across the restart",
        });
    }
    Ok(())
}

/// A park with nowhere to record is refused before anything runs: no store,
/// or a store that does not survive its writer.
#[test]
fn an_approval_without_a_store_is_refused() -> Result<(), ApprovalTestError> {
    let bare = Host::builder("acme")?.build()?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    match lgwks_bot::block_on(bare.run(&shipping, b"nowhere".to_vec())).error() {
        Some(error) if matches!(*error, FlowError::Store { .. }) => {}
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "a park with no store is a store refusal",
            });
        }
    }
    Ok(())
}

/// The same refusal over a store that cannot survive its writer.
#[cfg(feature = "testing")]
#[test]
fn an_approval_over_an_ephemeral_store_is_refused() -> Result<(), ApprovalTestError> {
    use crate::memory_records::MemoryRecords;
    let ephemeral = Host::builder("acme")?
        .records(std::sync::Arc::new(MemoryRecords::default()))
        .build()?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    match lgwks_bot::block_on(ephemeral.run(&shipping, b"nowhere".to_vec())).error() {
        Some(error) if matches!(*error, FlowError::Store { .. }) => Ok(()),
        _ => Err(ApprovalTestError::Setup {
            what: "a park over an ephemeral store is a store refusal",
        }),
    }
}

/// APR-3: a decision against other bytes is a mismatch, and the action it
/// would have released never runs.
#[test]
fn a_decision_against_other_bytes_is_a_mismatch_and_runs_nothing() -> Result<(), ApprovalTestError>
{
    let (_scratch, host, guarding) = release_fixture("approval-mismatch")?;
    let payload = b"release-44".to_vec();
    let report = lgwks_bot::block_on(host.run(&guarding, payload.clone()));
    let id = parked_id(&report)?;
    // A second park over other bytes: its digest is not this one's.
    let other = lgwks_bot::block_on(host.run(&guarding, b"other-bytes".to_vec()));
    let other_id = parked_id(&other)?;
    let other_digest = listed_digest(&host, &other_id)?;
    match host.decide(&id, ApprovalDecision::approve("mistaken", other_digest)) {
        Err(DecisionError::DigestMismatch { .. }) => {}
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "other bytes are a DigestMismatch",
            });
        }
    }
    let run = run_of(&report)?;
    // No decision was recorded, so the resume parks again under the same id.
    let again = lgwks_bot::block_on(host.resume(run, &guarding, payload));
    if !matches!(again.disposition(), Disposition::Parked) || parked_id(&again)? != id {
        return Err(ApprovalTestError::Setup {
            what: "a mismatched park is still undecided",
        });
    }
    if RELEASED.load(Ordering::SeqCst) != 0 {
        return Err(ApprovalTestError::Setup {
            what: "the refused action never ran",
        });
    }
    Ok(())
}

/// APR-4: a decision for another tenant's approval is refused, and the
/// attempt is readable in this host's audit.
#[test]
fn a_foreign_decision_is_refused_and_audited() -> Result<(), ApprovalTestError> {
    let scratch = scratch("approval-foreign")?;
    let path = scratch.path().join("shared.runstore");
    let store_a = RunStore::open(&path)?;
    let store_b = store_a.clone();
    let host_a = Host::builder("acme")?.store(store_a).build()?;
    let host_b = Host::builder("zed")?.store(store_b).build()?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let report = lgwks_bot::block_on(host_a.run(&shipping, b"release-45".to_vec()));
    let id = parked_id(&report)?;
    match host_b.decide(
        &id,
        ApprovalDecision::approve("zed", *host_a.pending(100)?[0].digest()),
    ) {
        Err(DecisionError::ForeignTenant { .. }) => {}
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "another tenant's approval is ForeignTenant",
            });
        }
    }
    let audit = host_b.audit();
    if audit.len() != 1 || audit[0].owner() != "acme" || audit[0].id() != id {
        return Err(ApprovalTestError::Setup {
            what: "the refused attempt is audited",
        });
    }
    if host_a.pending(100)?.len() != 1 {
        return Err(ApprovalTestError::Setup {
            what: "the foreign attempt decided nothing",
        });
    }
    Ok(())
}

/// APR-5: a resume past the deadline fails typed, records the lapse, and a
/// later decision reports the expiry.
#[test]
fn a_lapsed_approval_fails_typed_and_decides_expired() -> Result<(), ApprovalTestError> {
    let scratch = scratch("approval-expiry")?;
    let host = virtual_host_on(scratch.path(), "acme")?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let payload = b"release-46".to_vec();
    let (id, run, digest) = park_listed(&host, &shipping, &payload)?;
    lapse(&host)?;
    match lgwks_bot::block_on(host.resume(run, &shipping, payload)).error() {
        Some(error) if matches!(*error, FlowError::ApprovalExpired { .. }) => {}
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "a resume past the deadline fails as expired",
            });
        }
    }
    // The lapse is recorded, so the ceiling drops the approval and no later
    // listing names it.
    if !host.pending(100)?.is_empty() {
        return Err(ApprovalTestError::Setup {
            what: "a lapsed approval leaves the listing",
        });
    }
    match host.decide(&id, ApprovalDecision::approve("late", digest)) {
        Err(DecisionError::Expired { .. }) => Ok(()),
        _ => Err(ApprovalTestError::Setup {
            what: "a decision past the deadline is Expired",
        }),
    }
}

/// APR-6: a park past the tenant ceiling is refused at admission, before the
/// payload block runs.
#[test]
fn a_park_past_the_ceiling_is_refused_before_the_body_runs() -> Result<(), ApprovalTestError> {
    clear_noted();
    let scratch = scratch("approval-ceiling")?;
    let host = Host::builder("acme")?
        .run_store(scratch.path())?
        .max_pending_approvals(1)
        .build()?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let first = lgwks_bot::block_on(host.run(&shipping, b"one".to_vec()));
    if !matches!(first.disposition(), Disposition::Parked) {
        return Err(ApprovalTestError::Setup {
            what: "the first park lands",
        });
    }
    // A second run parks a different step key under the same ceiling.
    let second = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let parked = lgwks_bot::block_on(host.run(&second, b"two".to_vec()));
    let Some(failed) = parked.error() else {
        return Err(ApprovalTestError::Setup {
            what: "a park past the ceiling is refused",
        });
    };
    match *failed {
        FlowError::Store { ref source, .. } => match **source {
            StoreError::Limit {
                kind: StoreLimitKind::PendingApprovals,
                ..
            } => {}
            _ => {
                return Err(ApprovalTestError::Setup {
                    what: "a park past the ceiling names PendingApprovals",
                });
            }
        },
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "a park past the ceiling is refused",
            });
        }
    }
    if noted().len() != 1 {
        return Err(ApprovalTestError::Setup {
            what: "the refused park never ran its payload block",
        });
    }
    Ok(())
}

/// APR-7: under the default policy, an external act with no approval parked
/// is refused before it runs.
#[test]
fn an_external_act_without_an_approval_is_refused() -> Result<(), ApprovalTestError> {
    RELEASED.store(0, Ordering::SeqCst);
    let scratch = scratch("approval-gate")?;
    let host = release_host_on(scratch.path(), "acme")?;
    let acting = task("direct", |scope: Scope, payload: Vec<u8>| async move {
        direct(&scope, payload).await
    })?;
    match lgwks_bot::block_on(host.run(&acting, b"unapproved".to_vec())).error() {
        Some(error) if matches!(*error, FlowError::NeedsApproval { .. }) => {}
        _ => {
            return Err(ApprovalTestError::Setup {
                what: "an unapproved external act is NeedsApproval",
            });
        }
    }
    if RELEASED.load(Ordering::SeqCst) != 0 {
        return Err(ApprovalTestError::Setup {
            what: "the refused action never ran",
        });
    }
    // An explicit policy runs the stub without asking: the doubles are the
    // suite's own, so no decision is owed.
    let allowed = lgwks_bot::block_on(allowing(host.run(&acting, b"allowed".to_vec())));
    if !matches!(allowed.disposition(), Disposition::Succeeded) {
        return Err(ApprovalTestError::Setup {
            what: "an explicit policy runs the stub",
        });
    }
    Ok(())
}

/// APR-8: a decided approval releases the exact bytes it was parked over,
/// and the gated action runs once on them.
#[test]
fn an_approved_act_runs_once_on_the_approved_bytes() -> Result<(), ApprovalTestError> {
    let (_scratch, host, guarding) = release_fixture("approval-match")?;
    let payload = b"release-47".to_vec();
    let (id, run, digest) = park_listed(&host, &guarding, &payload)?;
    host.decide(&id, ApprovalDecision::approve("opener", digest))?;
    let resumed = lgwks_bot::block_on(host.resume(run, &guarding, payload.clone()));
    if !matches!(resumed.disposition(), Disposition::Succeeded)
        || resumed.output() != Some(&payload)
    {
        return Err(ApprovalTestError::Setup {
            what: "the approved run returns the approved bytes",
        });
    }
    if RELEASED.load(Ordering::SeqCst) != 1 {
        return Err(ApprovalTestError::Setup {
            what: "the gated action ran exactly once",
        });
    }
    if noted() != [payload] {
        return Err(ApprovalTestError::Setup {
            what: "the payload block ran once across the resume",
        });
    }
    Ok(())
}

/// APR-9: two threads racing one decision record exactly one; the other is
/// refused as already decided.
#[test]
fn two_racing_decisions_record_exactly_one() -> Result<(), ApprovalTestError> {
    let scratch = scratch("approval-race")?;
    let host = std::sync::Arc::new(host_on(scratch.path(), "acme")?);
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let report = lgwks_bot::block_on(host.run(&shipping, b"release-48".to_vec()));
    let id = parked_id(&report)?;
    let pending = host.pending(100)?;
    let digest = *pending
        .iter()
        .find(|entry| entry.id() == id)
        .map(|entry| entry.digest())
        .ok_or(ApprovalTestError::Setup {
            what: "the park is listed",
        })?;
    let decision = |by: &'static str| {
        let host = std::sync::Arc::clone(&host);
        let id = id.clone();
        // The verdict is read inside the racing thread: only the outcome
        // crosses the join, so the test needs nothing `Send` beyond the host.
        std::thread::Builder::new()
            .name(format!("approval-racer-{by}"))
            .spawn(
                move || match host.decide(&id, ApprovalDecision::approve(by, digest)) {
                    Ok(()) => "recorded",
                    Err(DecisionError::AlreadyDecided { .. }) => "decided",
                    Err(_) => "other",
                },
            )
    };
    let first = decision("racer-a")?;
    let second = decision("racer-b")?;
    let mut outcomes = [
        first.join().map_err(|_| ApprovalTestError::Setup {
            what: "the first racer joins",
        })?,
        second.join().map_err(|_| ApprovalTestError::Setup {
            what: "the second racer joins",
        })?,
    ];
    outcomes.sort();
    assert_eq!(
        outcomes,
        ["decided", "recorded"],
        "exactly one racer records the decision"
    );
    Ok(())
}
