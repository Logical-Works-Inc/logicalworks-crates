//! The host-stop family: a stop or a refusal before admission is the host declining, never the request's recorded verdict.

use super::*;

/// A host stopped mid-run does not record the stop as the request's outcome, so
/// the request is still completable and a later submission of the same key
/// re-drives the body from its recorded step.
#[test]
fn a_host_stop_mid_run_leaves_the_request_resumable() -> TestResult {
    let scratch = Scratch::new("req-stopped")?;
    let key = RequestKey::new("interrupted")?;
    let counter = request::FirstStepRuns::new();
    // The first run: a body that waits forever after its first durable step, so
    // the host's stop is the only thing that can end it.
    let host = stored_host("acme", scratch.path())?;
    let recorded = Arc::new(AtomicBool::new(false));
    let never = Arc::new(AtomicBool::new(false));
    let parked = two_step_task(Arc::clone(&recorded), Arc::clone(&never), counter.clone())?;

    let stopped = stop_mid_run(&host, &key, &parked, 11u32, &recorded)?;
    assert_eq!(
        disposition_of(&stopped)?,
        Disposition::Cancelled,
        "the caller sees the host's stop: a stop arrived after admission, so the run was admitted \
         and then cancelled"
    );
    let run = stopped.run_id().ok_or("the stopped run names a run")?;
    let stopped_records = host
        .run_store()
        .ok_or("a stored host keeps a store")?
        .record_count(run);
    assert!(
        stopped_records >= 2,
        "the stop left the receipt and the first step's record, saw {stopped_records}"
    );
    assert_eq!(
        counter.count(),
        1,
        "the stopped run performed the first effect exactly once"
    );

    // A host that did not stop, over the *reopened* file, sees a request with a
    // receipt and no terminal record — which is the only shape a resume can
    // start from.
    let reopened = stored_host("acme", scratch.path())?;
    let later = lgwks_bot::script::block_on(reopened.submit(
        &key,
        &work_two_step(counter.clone())?,
        11u32,
    ))?;
    let after_submit = counter.count();
    match later {
        Submission::InFlight(in_flight) => {
            assert_eq!(
                in_flight.run(),
                run,
                "the in-flight request names the same run"
            );
            assert_eq!(
                in_flight.task(),
                "two-step",
                "the in-flight report names the task"
            );
            assert!(
                in_flight.records() >= 2,
                "the receipt and the first step's record survive the stop, saw {}",
                in_flight.records()
            );
        }
        other => {
            return Err(format!(
                "a host stop reattached as {:?}; a request whose run was stopped has no verdict of \
                 its own to reattach to",
                other.report().map(lgwks_bot::task::Report::disposition)
            )
            .into());
        }
    }
    assert_eq!(
        after_submit, 1,
        "a submission of an in-flight request reports the uncertainty without re-running the body"
    );

    // A resume drives it to completion. The first step's record is on the disk,
    // so its effect must not happen a second time.
    let settled =
        lgwks_bot::script::block_on(reopened.resume(run, &work_two_step(counter.clone())?, 11u32));
    assert_eq!(
        settled.disposition(),
        Disposition::Succeeded,
        "the resumed run completes"
    );
    assert_eq!(
        settled.output().copied(),
        Some(11),
        "the resumed run answers the input"
    );
    assert_eq!(
        counter.count(),
        1,
        "the first effect happened once across the stop, the in-flight report and the resume"
    );
    Ok(())
}

/// A body of the shape that lets a run go on to its second step, sharing
/// `counter` so the test can read how many times the first effect ran.
///
/// # Errors
///
/// [`FlowError::InvalidName`] for the fixed name.
fn work_two_step(
    counter: request::FirstStepRuns,
) -> Result<
    lgwks_bot::task::Task<impl Fn(lgwks_bot::script::Scope, u32) -> request::BodyFuture>,
    FlowError,
> {
    two_step_task(
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
        counter,
    )
}

/// A refusal before admission is the host declining to finish the request, so it
/// records no terminal outcome and never reattaches as a verdict.
#[test]
fn a_refusal_before_admission_is_not_recorded_as_the_outcome() -> TestResult {
    let scratch = Scratch::new("req-refused")?;
    let store = RunStore::open(scratch.path().join("shared.runstore"))?;
    let stopping = host_on("acme", store.clone())?;
    let healthy = host_on("acme", store)?;
    let counter = Rc::new(Cell::new(0));
    let work = counting_task(Rc::clone(&counter))?;
    let key = RequestKey::new("refused-before-admission")?;

    // A host that is already stopped refuses before admission, so no body ever
    // runs. The receipt is still written: it binds the key to this input, which
    // is what makes the refusal this one call's and not the request's.
    stopping.cancel();
    let refused = lgwks_bot::script::block_on(stopping.submit(&key, &work, 6u32))?;
    assert_eq!(
        disposition_of(&refused)?,
        Disposition::Refused,
        "a stopped host refuses before admission rather than cancelling a run that never started"
    );
    assert_eq!(counter.get(), 0, "no body ran for a refused admission");

    // The healthy host submits the same key and the same input. The key is bound
    // to the input by the receipt the refused call wrote, so the healthy host
    // takes the same door a reattach takes — and finds no recorded verdict,
    // because a refusal is the host's and not the request's.
    let seen = lgwks_bot::script::block_on(healthy.submit(&key, &work, 6u32))?;
    let run = seen.run_id().ok_or("the healthy submission names a run")?;
    let in_flight = match seen {
        Submission::InFlight(in_flight) => in_flight,
        other => {
            return Err(format!(
                "a refusal before admission reattached as {:?}; a key nothing ever completed \
                 must stay completable",
                other.report().map(lgwks_bot::task::Report::disposition)
            )
            .into());
        }
    };
    assert_eq!(
        in_flight.run(),
        run,
        "the in-flight request names the derived run"
    );
    assert_eq!(
        in_flight.records(),
        1,
        "the receipt is the only record the refused call wrote: no step ran, so no step was \
         recorded, and no terminal verdict"
    );
    assert_eq!(
        counter.get(),
        0,
        "the healthy host did not run the body either"
    );

    // A resume drives it on the host that can admit it. This is the control on
    // the other side: what *is* the request's own verdict is recorded, and a
    // repeat reattaches to it.
    let settled = lgwks_bot::script::block_on(healthy.resume(run, &work, 6u32));
    assert_eq!(
        settled.disposition(),
        Disposition::Succeeded,
        "the healthy host runs the request to its own verdict"
    );
    assert_eq!(
        counter.get(),
        1,
        "the body ran once, on the host that admitted it"
    );
    let repeat = lgwks_bot::script::block_on(healthy.submit(&key, &work, 6u32))?;
    assert!(
        matches!(repeat, Submission::Reattached(_)),
        "a success is the request's verdict, so a repeat reattaches to it"
    );
    assert_eq!(repeat.run_id(), Some(run), "a reattach names the same run");
    assert_eq!(counter.get(), 1, "the reattach ran no body");
    Ok(())
}
