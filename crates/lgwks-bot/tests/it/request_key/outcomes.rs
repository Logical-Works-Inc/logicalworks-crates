//! The outcome family: a failure or an expired deadline the request itself produced is recorded, and a store refusal of the verdict is reported.

use super::*;

/// A failure reason whose archived `@terminal` record is refused by the store's
/// per-record byte ceiling.
///
/// The ceiling itself, not some large number: the archived record carries this
/// reason plus the disposition code, the step path and the framing head, so a
/// reason at the ceiling is already past what one record may carry, and the
/// smallest refusal is the one that proves the *declared bound* is what refused
/// the write rather than some other accident of size.
const OVER_CEILING_REASON: usize = lgwks_bot::task::MAX_RECORD_BYTES;

/// A store that refuses the `@terminal` write is reported as `Failed`, not
/// returned as a success nobody can find later.
///
/// Recording an outcome and reporting success are one fact (INV-BOT-102), so
/// the resume that *reached* a verdict but could not record it reports the
/// refusal — and, critically, records nothing: the request stays unsettled, so a
/// later client is told `InFlight` rather than being handed a verdict that was
/// never written. This is the error path through `Host::resume`, and it had no
/// coverage at all.
///
/// The refusal is real rather than mocked: the run's failure reason is archived
/// into the `@terminal` record, and one byte past the store's declared
/// per-record ceiling makes the framing refuse with the typed ceiling error.
#[test]
fn a_store_that_refuses_the_terminal_write_reports_the_refusal() -> TestResult {
    let scratch = Scratch::new("req-ceiling")?;
    let store = RunStore::open(scratch.path().join("shared.runstore"))?;
    let parking = host_on("acme", store.clone())?;
    let settling = host_on("acme", store.clone())?;
    let recorded = Arc::new(AtomicBool::new(false));
    let parked = parking_task(Arc::clone(&recorded))?;
    let key = RequestKey::new("verdict-the-store-will-refuse")?;

    // The first submission leaves the request in flight: the body parks after
    // its first durable record, so there is a receipt, a record, and no verdict.
    drop_after_first_step(Box::pin(parking.submit(&key, &parked, 1u32)), &recorded);
    let seen = lgwks_bot::script::block_on(settling.submit(&key, &parked, 1u32))?;
    let run = match seen {
        Submission::InFlight(in_flight) => in_flight.run(),
        other => {
            return Err(format!(
                "a dropped waiter must leave the request in flight; got {other:?}"
            )
            .into());
        }
    };

    // A body whose failure reason is at the ceiling a terminal record may carry,
    // so the settle-time write is refused by the store's own bound.
    let oversize = "r".repeat(OVER_CEILING_REASON);
    let oversize_task = task(
        "oversize",
        move |_scope: lgwks_bot::script::Scope, _: u32| {
            let oversize = oversize.clone();
            async move { Err::<u32, _>(FlowError::failed(oversize)) }
        },
    )?;

    let settled = lgwks_bot::script::block_on(settling.resume(run, &oversize_task, 1u32));
    assert_eq!(
        settled.disposition(),
        Disposition::Failed,
        "a run that reached a verdict the store refused to record is Failed, not Succeeded"
    );
    let error = settled.error().ok_or("the refusal names its failure")?;
    let rendered = error.to_string();
    assert!(
        rendered.contains("was not recorded"),
        "the report names that the outcome was not recorded: {rendered}"
    );
    assert!(
        rendered.contains("record's bytes"),
        "the report names the ceiling that refused it, rather than only that something failed: \
         {rendered}"
    );

    // Nothing was recorded as the verdict, so the request is still unsettled: a
    // reopen reports `InFlight` rather than handing a later client a verdict
    // that was never written.
    let reopened = host_on("acme", store)?;
    let after = lgwks_bot::script::block_on(reopened.submit(&key, &oversize_task, 1u32))?;
    assert!(
        matches!(after, Submission::InFlight(_)),
        "a request whose verdict the store refused is still unsettled, not reattached to; got {:?}",
        after.report().map(lgwks_bot::task::Report::disposition)
    );
    Ok(())
}

/// A failure the *body* produced is the request's verdict, so it is recorded:
/// the control on the two tests above.
#[test]
fn a_failed_run_is_the_requests_recorded_outcome() -> TestResult {
    let scratch = Scratch::new("req-failed")?;
    let host = stored_host("acme", scratch.path())?;
    let counter = Rc::new(Cell::new(0));
    let refusal = task(
        "failing",
        |_scope: lgwks_bot::script::Scope, value: u32| async move {
            if value == 0 {
                Err(FlowError::failed("the body refused this input"))
            } else {
                Ok(value)
            }
        },
    )?;
    let key = RequestKey::new("refused-by-the-body")?;

    let first = lgwks_bot::script::block_on(host.submit(&key, &refusal, 0u32))?;
    assert_eq!(
        disposition_of(&first)?,
        Disposition::Failed,
        "the body's own error is a failure, not a stop"
    );
    let run = first.run_id().ok_or("the failed run names a run")?;

    let again = lgwks_bot::script::block_on(host.submit(&key, &refusal, 0u32))?;
    assert!(
        matches!(again, Submission::Reattached(_)),
        "a recorded failure is the request's outcome, so a repeat reattaches to it"
    );
    assert_eq!(
        again.report().map(lgwks_bot::task::Report::disposition),
        Some(Disposition::Failed),
        "the reattached report carries the recorded failure"
    );
    assert_eq!(again.run_id(), Some(run), "a reattach names the same run");
    assert_eq!(
        counter.get(),
        0,
        "the counter task was never submitted under this key"
    );
    Ok(())
}

/// The declared deadline expiring mid-run is the request's own verdict, so it is
/// recorded: the control that separates it from a host stop.
///
/// The two dispositions differ in the way that matters to a key. A stop is the
/// host declining to finish the run, so it records nothing and the key stays
/// completable. A deadline is the run reaching the budget its own declaration
/// fixed, which is a fact about the request and not about this host — so it is
/// recorded, and a later client of the same key reattaches to it instead of
/// re-running a body that has already overrun once.
#[test]
fn an_expired_deadline_is_the_requests_recorded_outcome() -> TestResult {
    let scratch = Scratch::new("req-deadline")?;
    let runs = request::FirstStepRuns::new();
    let budget = Duration::from_millis(50);
    let host = host_with_deadline("acme", scratch.path(), budget)?;
    let work = overrunning_task(runs.clone())?;
    let key = RequestKey::new("overruns-its-budget")?;

    let first = lgwks_bot::script::block_on(host.submit(&key, &work, 3u32))?;
    assert_eq!(
        disposition_of(&first)?,
        Disposition::DeadlineExceeded,
        "a body that outruns its declared budget is DeadlineExceeded"
    );
    assert_eq!(
        runs.count(),
        1,
        "the first durable step ran and recorded before the deadline fired"
    );
    let run = first.run_id().ok_or("the overrunning run names a run")?;

    let again = lgwks_bot::script::block_on(host.submit(&key, &work, 3u32))?;
    assert!(
        matches!(again, Submission::Reattached(_)),
        "the recorded deadline is the request's outcome, so a repeat reattaches to it; got {:?}",
        again.report().map(lgwks_bot::task::Report::disposition)
    );
    assert_eq!(
        again.report().map(lgwks_bot::task::Report::disposition),
        Some(Disposition::DeadlineExceeded),
        "the reattached report carries the recorded deadline"
    );
    assert_eq!(again.run_id(), Some(run), "a reattach names the same run");
    assert_eq!(
        runs.count(),
        1,
        "the reattach never entered the body again: a request that already overran \
         its budget must not overrun it a second time"
    );
    Ok(())
}
