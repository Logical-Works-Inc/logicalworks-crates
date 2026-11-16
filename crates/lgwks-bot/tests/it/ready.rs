//! Black-box acceptance for typed readiness (T18 / LC-09).
//!
//! Every journey here drives the **public** surface: a task body on a
//! [`Host`], a [`Readiness`] from `lgwks_bot::script`, and — for the real-process
//! journey — a supervised child whose own stdout line closes the gate. Nothing
//! reaches past a public door, and every assertion is on the exact observation
//! the row names rather than on an internal counter.
//!
//! | Test | What it pins |
//! |---|---|
//! | `a_failure_before_ready_reaches_the_dependant_and_the_report_t18` | a service that never becomes ready fails the dependant at its own path, and the caller's `Report` carries it |
//! | `a_duplicate_ready_signal_is_refused_and_releases_nothing` | a second release is a typed refusal, not a second release |
//! | `a_stale_generation_is_refused_and_releases_nothing` | an older instance cannot release dependants onto a newer one, and a foreign generation is a *different* refusal |
//! | `a_restarted_service_arms_a_new_readiness` | the restart shape end to end |
//! | `a_failure_after_ready_cancels_every_dependant_still_running` | the dependants still running are told, and each report says so |
//! | `a_shutdown_is_a_stop_and_not_a_failure` | teardown is not reported as an outage |
//! | `a_wait_is_budgeted_cancellable_and_immediate_when_already_released` | the wait has a budget, obeys the stop, and costs nothing when the answer exists |
//! | `dependants_are_capped_at_the_declared_bound` | the cap is charged before a slot and refuses rather than growing |
//! | `no_readiness_path_sleeps_or_polls` | the module's own source holds no sleep and no polling loop |
//! | `a_child_printing_its_address_releases_the_dependants` | the real-process journey: readiness from a child's stdout, not from a timer |
//! | `a_child_that_dies_without_printing_never_releases_the_dependants` | a child that never announces is never released |
//! | `a_child_that_dies_after_announcing_stops_the_dependants` | the failure-after-ready journey, driven by a real exit rather than a signal |
#![cfg(feature = "script")]

use std::error::Error;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lgwks_bot::script::{FlowError, Generation, Readiness, ReadinessError, Scope, Tenant};
use lgwks_bot::task::{Disposition, Host, Report, Task, task};

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// A readiness carrying a `String`, the shape every journey here uses: the
/// evidence a dependant receives is whatever the service published, and a
/// `String` is what a child prints.
type Named = Readiness<String>;

/// The budget a journey's wait runs under.
///
/// Long enough that no journey is decided by it and short enough that a journey
/// whose wait is not actually bounded fails in milliseconds rather than at the
/// harness's own timeout.
const BUDGET: Duration = Duration::from_millis(500);

/// A root scope for `tenant`.
fn scope_of(tenant: &str) -> Result<Scope, Box<dyn Error>> {
    Ok(Scope::root(Tenant::new(tenant)?))
}

/// Drive one future on a fresh current-thread runtime.
fn drive<T>(future: impl Future<Output = T>) -> T {
    lgwks_bot::script::block_on(future)
}

/// Assert a report failed, naming the path the failure was located at.
fn assert_failed_at<O>(report: &Report<O>, path: &str, label: &str) {
    assert_eq!(
        report.disposition(),
        Disposition::Failed,
        "{label}: the run must be Failed, got {:?}",
        report.error()
    );
    let located = report.error().map(FlowError::at);
    // A `Failed` report with no located error is not a located failure, and
    // checking a stand-in path would pass for the wrong reason.
    assert!(
        located.is_some(),
        "{label}: a Failed report must carry the error that failed it"
    );
    let Some(at) = located else {
        return;
    };
    assert!(
        at.ends_with(path),
        "{label}: the failure must be located at a path ending in {path:?}, got {at:?}"
    );
}

/// The error of a result that must have been refused, or a typed failure saying
/// it was not.
///
/// `Result::expect_err` is banned here — and rightly, because it *panics*. So is
/// a hand-written `panic!`, and so is any other shape that turns "this must have
/// been refused" into a panic rather than a report. This returns
/// `Result<E, Accepted>`, where `Accepted` names the case *and* carries the value
/// that was wrongly accepted, so the journey propagates it with `?` and the
/// harness prints it — the same path every other failure in this file takes.
fn refused<T: std::fmt::Debug, E>(outcome: Result<T, E>, what: &str) -> Result<E, Accepted> {
    match outcome {
        Err(error) => Ok(error),
        Ok(accepted) => Err(Accepted::of(what, &accepted)),
    }
}

/// A result that was accepted where a refusal was required.
///
/// An `Error` so the journey's own `?` propagates it; `Display` so the harness
/// prints what was expected and what happened instead. `what` is carried rather
/// than formatted at the point of failure so the message survives the `?` that
/// delivers it.
#[derive(Debug, PartialEq, Eq)]
struct Accepted {
    /// What the journey required to be refused.
    what: String,
    /// What was accepted instead, rendered.
    accepted: String,
}

impl Accepted {
    /// Render the accepted value into this failure's text.
    fn of<T: std::fmt::Debug>(what: &str, accepted: &T) -> Self {
        Self {
            what: what.to_owned(),
            accepted: format!("{accepted:?}"),
        }
    }
}

impl std::fmt::Display for Accepted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: this must have been refused, but it was accepted as {}",
            self.what, self.accepted
        )
    }
}

impl Error for Accepted {}

/// A `Task` whose body waits on `readiness` at `step`, so a journey's assertion
/// is on the disposition and located error a caller reads rather than on the
/// readiness's own state.
fn waiter(
    step: Scope,
    readiness: Named,
) -> Result<Task<impl Fn(Scope, ()) -> WaitBody>, FlowError> {
    task(
        "use-service",
        move |_scope: Scope, _input: ()| -> WaitBody {
            // Both captured values are cloned into the body, so the closure
            // itself stays `Fn`: a `Task`'s body may be called more than once,
            // and a closure that gave its capture away on the first call could
            // not be one.
            let readiness = readiness.clone();
            let step = step.clone();
            Box::pin(async move {
                readiness
                    .wait(&step, BUDGET)
                    .await
                    .map(|ready| ready.awaited().to_owned())
            })
        },
    )
}

/// The future a waiter's body returns.
///
/// Named rather than left to inference so the waiter's signature states what a
/// dependant produces: a `String`, or a located `FlowError`. A closure's own
/// future type is unnameable, and an unnamed return type is a second definition
/// of what this body returns that no reader can check.
type WaitBody = Pin<Box<dyn Future<Output = Result<String, FlowError>>>>;

/// How many dependants the failure-after-ready journey runs at once.
///
/// Three rather than one: a failure that reaches "a dependant" and a failure that
/// reaches *every* dependant are different facts, and only more than one proves
/// the readiness does not stop at the first token it happens to hold.
const DEPENDANTS: usize = 3;

// ── Readiness failure reaches the caller ────────────────────────────────────

/// A service that fails before it is ready fails the dependant at its own path,
/// and the caller's `Report` carries that failure with the service's reason.
#[test]
fn a_failure_before_ready_reaches_the_dependant_and_the_report_t18() -> TestResult {
    let scope = scope_of("acme")?;
    let readiness: Named = Readiness::new("db")?;
    let host = Host::builder("acme")?.build()?;
    let declared = waiter(scope.enter("use-db")?, readiness.clone())?;

    // Nothing has been signalled and nothing has failed, so a reading taken now
    // is `None` — the fact a caller checks before it decides to wait.
    assert!(
        readiness.outcome().is_none(),
        "an unsignalled readiness reports no outcome"
    );

    readiness.fail(Generation::FIRST, "could not bind port 5432")?;

    let report = drive(host.run(&declared, ()));
    assert_failed_at(
        &report,
        "use-db/await-ready",
        "a service that never came up",
    );
    let Some(reported) = report.error() else {
        let refusal = Err("a failed report must carry the error that failed it".into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "a_failure_before_ready_reaches_the_dependant_and_the_report_t18: returning an error to the caller");
        return refusal;
    };
    let error = reported.to_string();
    assert!(
        error.contains("could not bind port 5432"),
        "the report must carry the service's own reason, got {error:?}"
    );
    assert_eq!(
        readiness.admitted(),
        1,
        "the dependant was admitted exactly once, before it was answered"
    );
    // A failure before the release owes nobody a cancellation.
    assert!(
        readiness.failed_at().is_none(),
        "nothing was released, so this is not a failure after a release"
    );
    Ok(())
}

// ── Duplicate readiness ─────────────────────────────────────────────────────

/// A second ready signal is a typed refusal, and it releases nothing.
///
/// Both halves matter: the refusal must be *typed* (a caller can tell it from a
/// lost signal) and the already-released generation must survive it, so a
/// dependant arriving afterwards still reads the first release rather than the
/// value the duplicate offered.
#[test]
fn a_duplicate_ready_signal_is_refused_and_releases_nothing() -> TestResult {
    let scope = scope_of("acme")?;
    let readiness: Named = Readiness::new("db")?;
    readiness.ready(Generation::FIRST, "db.internal:5432".to_owned())?;

    let duplicate = refused(
        readiness.ready(Generation::FIRST, "db.elsewhere:5432".to_owned()),
        "a second release must be refused",
    )?;
    assert_eq!(
        duplicate,
        ReadinessError::AlreadyReady {
            what: Arc::from("db"),
            generation: Generation::FIRST,
        },
        "a duplicate names the readiness and the generation already released"
    );
    assert!(!duplicate.released(), "a refused duplicate released nobody");
    assert!(
        duplicate.is_permanent(),
        "repeating a duplicate release yields the same refusal, so an enclosing \
         retry must not treat it as transient"
    );
    assert_eq!(
        readiness.released_at(),
        Some(Generation::FIRST),
        "the released generation survives the duplicate"
    );

    // A dependant that arrives after the duplicate still reads the *first*
    // release, not the value the duplicate offered.
    let ready = drive(readiness.wait(&scope, BUDGET))?;
    assert_eq!(
        ready.awaited(),
        "db.internal:5432",
        "a late dependant reads the first release, not the refused duplicate"
    );
    assert_eq!(ready.generation(), Generation::FIRST);
    Ok(())
}

// ── Stale generation ────────────────────────────────────────────────────────

/// A signal from an older instance releases nothing, and a signal from a
/// generation this readiness never issued is a *different* refusal.
///
/// A restarted service's previous instance may still hold a handle and still
/// decide to say "ready". The generation it carries is behind the one the
/// readiness is bound to, and the readiness must refuse it rather than release
/// dependants onto a process that has been replaced.
#[test]
fn a_stale_generation_is_refused_and_releases_nothing() -> TestResult {
    let scope = scope_of("acme")?;
    // Named rather than drawn: "stale" is an *ordering* against the bound
    // generation, so a test must be able to name an older one. Two consecutive
    // `next()` draws would be newer, not older. The base is far above zero so
    // `saturating_sub` is not silently clamped onto the bound generation, which
    // would make this journey assert nothing.
    const BASE: u64 = 1_000;
    let current = Generation::at(BASE);
    let older = Generation::at(BASE.saturating_sub(1));
    let readiness: Named = Readiness::named("db", current, 4)?;

    let stale = refused(
        readiness.ready(older, "db.old:5432".to_owned()),
        "a stale signal must be refused",
    )?;
    assert_eq!(
        stale,
        ReadinessError::StaleGeneration {
            what: Arc::from("db"),
            expected: current,
            got: older,
        },
        "a stale signal names the generation expected and the one it carried"
    );
    assert!(!stale.released(), "a refused signal released nobody");
    assert!(
        readiness.stale(older),
        "the readiness agrees the older generation is stale"
    );
    assert!(
        !readiness.stale(current),
        "the bound generation is not stale"
    );
    assert!(
        readiness.outcome().is_none(),
        "a refused signal settled nothing, so no outcome is reported"
    );
    assert_eq!(
        readiness.admitted(),
        0,
        "a refused signal admitted no dependant"
    );

    // A generation this readiness never issued is a different fact: it is not a
    // previous instance, it is a signal from somewhere else entirely. Named from
    // the shared counter so it is drawn from the same space the readiness could
    // have been armed at, and strictly newer than the bound one — a value
    // *below* the bound is a stale instance, which is the case above.
    let stranger = Generation::at(BASE.saturating_add(1));
    assert_eq!(
        refused(
            readiness.ready(stranger, "db.stranger:5432".to_owned()),
            "a foreign generation must be refused",
        )?,
        ReadinessError::UnknownGeneration {
            what: Arc::from("db"),
            got: stranger,
        },
        "a generation this readiness never issued is refused as unknown, not stale"
    );

    // The bound generation is still accepted afterwards, so a refusal does not
    // poison the readiness into refusing everything.
    readiness.ready(current, "db.new:5432".to_owned())?;
    let ready = drive(readiness.wait(&scope, BUDGET))?;
    assert_eq!(
        ready.awaited(),
        "db.new:5432",
        "the current instance's release is what a dependant receives"
    );
    assert_eq!(ready.generation(), current);
    Ok(())
}

/// The restart shape end to end: the old instance's readiness stays settled at
/// its own generation while the new instance's readiness releases dependants.
#[test]
fn a_restarted_service_arms_a_new_readiness() -> TestResult {
    let scope = scope_of("acme")?;
    let first = Generation::at(7);
    let second = Generation::at(8);

    let before: Named = Readiness::named("db", first, 4)?;
    before.ready(first, "db.first:5432".to_owned())?;

    let after: Named = Readiness::named("db", second, 4)?;
    after.ready(second, "db.second:5432".to_owned())?;

    let ready = drive(after.wait(&scope, BUDGET))?;
    assert_eq!(
        ready.awaited(),
        "db.second:5432",
        "a dependant receives the restarted instance's own release"
    );
    assert_eq!(
        ready.generation(),
        second,
        "and the generation that restart armed"
    );
    // The old readiness is settled at its own generation and refuses the new
    // instance's generation: they are two readinesses, not one handle the
    // restarted instance may write through. The refusal is *unknown generation*
    // rather than *stale*, because 8 is newer than 7 — the signal is not from an
    // older instance, it is from a different readiness entirely, and a caller
    // told "stale" would look for a service to re-arm rather than for the handle
    // it is holding.
    assert_eq!(
        before.released_at(),
        Some(first),
        "the old readiness still reports its own release"
    );
    assert_eq!(
        refused(
            before.ready(second, "db.second:5432".to_owned()),
            "the new instance cannot write through the old readiness",
        )?,
        ReadinessError::UnknownGeneration {
            what: Arc::from("db"),
            got: second,
        },
        "a signal from a different readiness is refused as unknown, and the old \
         readiness keeps its own release"
    );
    Ok(())
}

// ── Failure after ready ─────────────────────────────────────────────────────

/// A failure *after* the release reaches every dependant that is still running.
///
/// The dependants here are real scopes that went through the readiness's own
/// `wait`, so the readiness holds each of their tokens by the fact of admitting
/// them. Each dependant then keeps working *past* the release — that is the
/// state the row is about, "failure after ready" — and the failure cancels
/// exactly those tokens, so each dependant observes a stop at its own step and
/// each report carries it rather than reporting a run that used a service which
/// had already died.
#[test]
fn a_failure_after_ready_cancels_every_dependant_still_running() -> TestResult {
    let readiness: Named = Readiness::new("db")?;
    readiness.ready(Generation::FIRST, "db.internal:5432".to_owned())?;
    let parked = Arc::new(AtomicUsize::new(0));

    // Three dependants, each in its own step. Each is admitted by waiting on
    // the readiness, which is what hands its token over, and each then parks on
    // work that outlives the release.
    let host = Host::builder("acme")?
        .max_concurrent_tasks(limit_of(DEPENDANTS)?)
        .build()?;
    let reached_in_body = Arc::clone(&parked);
    let body = {
        let readiness = readiness.clone();
        move |scope: Scope, input: usize| {
            let readiness = readiness.clone();
            let reached = Arc::clone(&reached_in_body);
            async move {
                let step = scope.enter(&format!("worker-{input}"))?;
                // Admitted: the readiness now holds this step's token.
                let ready = readiness.wait(&step, BUDGET).await?;
                assert_eq!(
                    ready.awaited(),
                    "db.internal:5432",
                    "dependant {input} was released with the published address"
                );
                // Still running when the service dies below.
                reached.fetch_add(1, Ordering::SeqCst);
                step.token().cancelled().await;
                step.checkpoint()?;
                Ok::<usize, FlowError>(input)
            }
        }
    };
    let declared: Task<_> = task("use-service", body)?;

    // Drive every run until all three are suspended inside their parked bodies.
    // A hand-driven loop rather than a delay, so the failure lands at a point
    // every dependant has actually reached rather than after a guess. The count
    // is read *after* the poll rather than before it: the last dependant to park
    // does so inside this loop, and checking first would return `Pending` on the
    // turn that observed it with nothing left to wake the loop.
    let mut runs: Vec<Pin<Box<_>>> = (0..DEPENDANTS)
        .map(|index| Box::pin(host.run(&declared, index)))
        .collect();
    drive(std::future::poll_fn(|context| {
        use std::task::Poll;
        let mut any_pending = false;
        for run in &mut runs {
            match run.as_mut().poll(context) {
                Poll::Ready(_) => return Poll::Ready(()),
                Poll::Pending => any_pending = true,
            }
        }
        if !any_pending || parked.load(Ordering::SeqCst) == DEPENDANTS {
            return Poll::Ready(());
        }
        Poll::Pending
    }));
    // Each token was registered by the readiness that released it, so the
    // readiness knows exactly which dependants a later failure must reach.
    assert_eq!(
        readiness.admitted(),
        u64::try_from(DEPENDANTS)?,
        "each dependant was admitted once, by the readiness's own wait"
    );
    assert_eq!(
        readiness.dependants_live(),
        DEPENDANTS,
        "the readiness holds one token per dependant it released"
    );

    readiness.fail(Generation::FIRST, "the service exited")?;
    let after = readiness
        .failed_at()
        .ok_or("a failure after the release must be recorded as one")?;
    assert_eq!(
        after.generation(),
        Generation::FIRST,
        "the recorded failure names the generation that was released"
    );
    assert_eq!(
        after.reason(),
        "the service exited",
        "and carries the service's own reason"
    );
    assert_eq!(
        readiness.dependants_live(),
        0,
        "a failure after the release took every token it reached, so a second \
         failure cancels nobody twice"
    );
    assert!(
        readiness
            .fail(Generation::FIRST, "the service exited")
            .is_err(),
        "a second failure for the same generation is refused rather than \
         re-applied"
    );

    // Awaited one after another rather than through a bounded join: each run is
    // already pinned, so awaiting it directly needs no `'static` borrow of the
    // host or the task, and the order the reports come back in is the order the
    // dependants were declared in — which is the order this assertion indexes.
    let reports: Vec<Report<usize>> = runs.iter_mut().map(|run| drive(run.as_mut())).collect();
    assert_eq!(reports.len(), DEPENDANTS, "every dependant reported");
    for (index, report) in reports.iter().enumerate() {
        assert_eq!(
            report.disposition(),
            Disposition::Cancelled,
            "dependant {index}: a failure after the release must stop the \
             dependant, got {:?}",
            report.error()
        );
    }
    Ok(())
}

/// A non-zero admission ceiling, for a host that runs several dependants at once.
fn limit_of(limit: usize) -> Result<NonZeroUsize, Box<dyn Error>> {
    NonZeroUsize::new(limit).ok_or_else(|| "the ceiling must be at least one".into())
}
#[test]
fn a_shutdown_is_a_stop_and_not_a_failure() -> TestResult {
    let scope = scope_of("acme")?;
    let readiness: Named = Readiness::new("db")?;
    readiness.shutdown(Generation::FIRST)?;

    let error = refused(
        drive(readiness.wait(&scope, BUDGET)).map(|_ready| ()),
        "a shut-down readiness must not release a dependant",
    )?;
    assert!(
        error.is_cancelled(),
        "a teardown is a stop, not a service failure: {error}"
    );
    assert!(
        !error.to_string().contains("did not become ready"),
        "a shutdown must not be reported as a failure to come up: {error}"
    );
    assert!(
        readiness.failed_at().is_none(),
        "a shutdown is not a failure, so it is not recorded as one"
    );
    assert_eq!(
        refused(
            readiness.ready(Generation::FIRST, "db.internal:5432".to_owned()),
            "a shut-down readiness refuses further signals",
        )?,
        ReadinessError::Closed {
            what: Arc::from("db"),
            generation: Generation::FIRST,
        },
        "a signal after a shutdown is refused by name"
    );
    Ok(())
}

// ── The real-process journey ────────────────────────────────────────────────

/// The real-process half. Gated on the same features as every other supervised
/// child journey in this suite, so a build without the `process` driver does not
/// compile a test it cannot run.
#[cfg(all(unix, feature = "process"))]
mod real_process;

mod source_scan;
mod waiting;
