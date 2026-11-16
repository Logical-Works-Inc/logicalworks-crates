//! The request rows: T15 replay and drift, T17 dropped waiters, T30 keys, payload conflicts and reattach.

use super::*;
#[cfg(feature = "ephemeral")]
use lgwks_bot::task::DefinitionIdentity;

// ── T15 ─────────────────────────────────────────────────────────────────────

/// T15, replay half: a duplicate submission replays the recorded answer
/// without re-entering the recorded step and without a new model request.
///
/// The run identity comes from a request-keyed submission, which derives it
/// from tenant and key rather than minting it, so no entropy source is needed.
/// The drift half needs minted runs and stays `ephemeral` below.
#[test]
fn a_recorded_answer_replays_without_a_new_request_t15() -> TestResult {
    let scratch = Scratch::new("t15-replay")?;
    let host = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let key = RequestKey::new("t15-replay")?;
    let entered = Rc::new(Cell::new(0_u32));

    let counted = counted_u32_task!("replay-a", &entered)?;
    let first = lgwks_bot::script::block_on(host.submit(&key, &counted, 5u32))?;
    assert!(
        matches!(first, Submission::Executed(_)),
        "the first submission executes and records its value"
    );
    assert_eq!(entered.get(), 1, "the first attempt recorded its step once");

    let dupe = lgwks_bot::script::block_on(host.submit(&key, &counted, 5u32))?;
    assert!(
        matches!(dupe, Submission::Reattached(_)),
        "a repeat under the same key and input reattaches"
    );
    let report = dupe
        .report()
        .ok_or("a reattach carries the recorded report")?;
    assert_eq!(
        report.output(),
        Some(&5),
        "the recorded answer comes back without a new request"
    );
    assert_eq!(
        entered.get(),
        1,
        "the reattach did not poll the recorded step again"
    );
    Ok(())
}

/// T15, drift half: a compatible resume replays without re-entering the
/// recorded step, and resuming the same run under a revised definition is a
/// typed `Incompatible` on the definition axis before any new effect.
///
/// Needs `ephemeral`: compatible resumes replay runs the entropy source
/// minted. Identities are declared explicitly rather than derived, so the
/// comparison the test asserts is the one the store actually ran. The resume
/// runs on a fresh host over the same directory: the claim is about a
/// restart, and a resume on the handle that wrote the records would prove only
/// that a map still had its entries. The revision axis — the name is a label,
/// not an identity, so renaming is not a drift and is not asserted as one.
/// The sweep file takes the input axis beside it.
#[cfg(feature = "ephemeral")]
#[test]
fn a_compatible_resume_replays_while_a_revised_definition_is_a_typed_drift_t15() -> TestResult {
    const CODEC: &str = "lgwks.bot.t15.v1";
    let scratch = Scratch::new("t15-drift")?;
    let host = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let entered = Rc::new(Cell::new(0_u32));

    let declared = host
        .definition("drift-a", 1, Some(input_digest_of(5)), 1)
        .with_codec(CODEC);
    let counted = counted_u32_task!("drift-a", &entered)?;
    let first: Report<u32> = lgwks_bot::script::block_on(host.run_under(&declared, &counted, 5u32));
    assert_eq!(
        first.disposition(),
        Disposition::Succeeded,
        "the first attempt records its value: {:?}",
        first.error()
    );
    assert_eq!(first.output(), Some(&5));
    let run = first.run_id().ok_or("a stored run names its run id")?;
    assert_eq!(entered.get(), 1, "the first attempt recorded its step once");
    drop(host);

    // The compatible resume on a fresh host finds its own records: same
    // output, recorded step not re-entered.
    let host = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let resumed: Report<u32> =
        lgwks_bot::script::block_on(host.resume_under(run, &declared, &counted, 5u32));
    assert_eq!(
        resumed.disposition(),
        Disposition::Succeeded,
        "a compatible resume replays to the recorded answer: {:?}",
        resumed.error()
    );
    assert_eq!(resumed.output(), Some(&5));
    assert_eq!(
        entered.get(),
        1,
        "the replay returned the recorded value without polling the recorded step again"
    );

    // The drifted resume: the same run under a revised definition disagrees on
    // the definition axis before any new effect runs.
    let revised = DefinitionIdentity::new("drift-a", 2, input_digest_of(5), 1).with_codec(CODEC);
    let drifted: Report<u32> =
        lgwks_bot::script::block_on(host.resume_under(run, &revised, &counted, 5u32));
    assert_eq!(
        incompatible_axis(drifted.error())?,
        "definition",
        "a revised definition drifts on the definition axis"
    );
    assert_eq!(entered.get(), 1, "a refused resume re-recorded nothing");
    Ok(())
}

// ── T17 ─────────────────────────────────────────────────────────────────────

/// T17: a dropped waiter leaves uncertainty (`InFlight`, no report) under the
/// run the key derives, and a later client settles that same run to a terminal.
/// Uncertainty and cleanup are two separate observations from the same host.
#[test]
fn a_dropped_waiter_leaves_uncertainty_and_a_later_client_settles_it_t17() -> TestResult {
    let scratch = Scratch::new("t17-drop")?;
    let host = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let key = RequestKey::new("order-17")?;

    // A body that signals entry, then parks: the shape a client that walks away
    // mid-effect leaves behind.
    let entered = Arc::new(AtomicBool::new(false));
    let parking = parking_task!("parking", &entered)?;
    drive_until_entered(Box::pin(host.submit(&key, &parking, 9u32)), &entered);
    assert!(
        entered.load(Ordering::SeqCst),
        "the waiter entered its body before being dropped"
    );

    // Uncertainty: the key is claimed, no terminal is recorded, and the
    // submission says so with no report attached.
    let uncertain = lgwks_bot::script::block_on(host.submit(&key, &parking, 9u32))?;
    assert!(
        uncertain.report().is_none(),
        "an in-flight submission carries no report: it is a statement that there is not one yet"
    );
    let in_flight_run = match uncertain {
        Submission::InFlight(in_flight) => in_flight.run(),
        other => {
            return Err(format!(
                "a dropped waiter leaves the request in flight, with no report: got {other:?}"
            )
            .into());
        }
    };

    // Cleanup: a later client settles the same run to a terminal outcome.
    // A resume, not a second submit: while the run is in flight the key still
    // reports uncertainty, and the resume is the handle that settles it.
    let settled_count = Rc::new(Cell::new(0_u32));
    let settler = counted_u32_task!("settler", &settled_count)?;
    let settled_report: Report<u32> =
        lgwks_bot::script::block_on(host.resume(in_flight_run, &settler, 9u32));
    assert_eq!(
        settled_report.disposition(),
        Disposition::Succeeded,
        "the later client settles the run: {:?}",
        settled_report.error()
    );
    assert_eq!(settled_report.output(), Some(&9));
    assert_eq!(
        settled_report.run_id(),
        Some(in_flight_run),
        "uncertainty and cleanup are about one run: the key derives it on every submission"
    );
    assert_eq!(settled_count.get(), 1, "the settling body ran once");
    Ok(())
}

/// T17, drain half: after a waiter is dropped mid-effect, the still-owning
/// host reports cleanup and uncertainty as two independent facts, and draining
/// it moves neither.
///
/// Cleanup is the admission counters: the dropped waiter's permit came back,
/// so nothing is in flight and every permit is free. Uncertainty is the
/// request: no terminal was recorded, so a later submission is told
/// `InFlight` with no report. A host that conflated the two would either keep
/// the permit to stand for the uncertain request (a leak a drain can never
/// clear) or drop the request with its permit (a lost effect reported as
/// cleaned up). After `cancel` drains the host, the counters are unchanged and
/// a second host over the same store still reads the request as in flight,
/// then settles it to a terminal under the same run.
#[test]
fn a_drained_host_reports_cleanup_and_uncertainty_as_two_facts_t17() -> TestResult {
    let scratch = Scratch::new("t17-drain")?;
    let owner = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let ceiling = owner.limits().max_concurrent_tasks();
    let key = RequestKey::new("order-17-drain")?;

    let entered = Arc::new(AtomicBool::new(false));
    let parking = parking_task!("parking", &entered)?;
    drive_until_entered(Box::pin(owner.submit(&key, &parking, 4u32)), &entered);
    assert!(
        entered.load(Ordering::SeqCst),
        "the waiter entered its body before being dropped"
    );

    // Cleanup, read from the admission counters alone.
    let admission = owner.admission();
    assert_eq!(
        admission.in_flight(),
        0,
        "the dropped waiter holds no permit"
    );
    assert_eq!(
        admission.available_permits(),
        ceiling,
        "every permit is free again: {admission:?}"
    );

    // Uncertainty, read from the request alone.
    let run = match lgwks_bot::script::block_on(owner.submit(&key, &parking, 4u32))? {
        Submission::InFlight(in_flight) => in_flight.run(),
        other => {
            let refusal =
                Err(format!("cleanup must not erase the uncertain request: got {other:?}").into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "a_drained_host_reports_cleanup_and_uncertainty_as_two_facts_t17: returning an error to the caller");
            return refusal;
        }
    };

    // Drain the owner. Its counters do not move, and the uncertainty outlives it.
    owner.cancel();
    assert!(owner.is_cancelled(), "the drain took effect");
    assert_eq!(
        owner.admission().in_flight(),
        0,
        "the drain freed nothing it had not freed"
    );
    assert_eq!(owner.admission().available_permits(), ceiling);
    drop(owner);

    let successor = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    match lgwks_bot::script::block_on(successor.submit(&key, &parking, 4u32))? {
        Submission::InFlight(in_flight) => assert_eq!(
            in_flight.run(),
            run,
            "the successor reads the same uncertain run the owner left"
        ),
        other => {
            let refusal = Err(format!(
                "a drain must not settle the request by omission: got {other:?}"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "a_drained_host_reports_cleanup_and_uncertainty_as_two_facts_t17: returning an error to the caller");
            return refusal;
        }
    }
    let settled = Rc::new(Cell::new(0_u32));
    let settler = counted_u32_task!("settler", &settled)?;
    let report: Report<u32> = lgwks_bot::script::block_on(successor.resume(run, &settler, 4u32));
    assert_eq!(
        report.disposition(),
        Disposition::Succeeded,
        "the successor settles the uncertain run: {:?}",
        report.error()
    );
    assert_eq!(report.run_id(), Some(run), "under the run the key derives");
    assert_eq!(settled.get(), 1, "the settling body ran once");
    assert_eq!(
        successor.admission().in_flight(),
        0,
        "and returned its permit"
    );
    Ok(())
}

// ── T30 ─────────────────────────────────────────────────────────────────────

/// T30: the same key with a different payload is a typed `Conflict`, a
/// duplicate identical request reattaches without re-entering the body, and
/// distinct keys are distinct runs.
#[test]
fn one_key_one_payload_conflict_reattach_and_distinct_keys_t30() -> TestResult {
    let scratch = Scratch::new("t30-keys")?;
    let host = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let counter = Rc::new(Cell::new(0_u32));
    let work = counted_u32_task!("counted", &counter)?;

    let key = RequestKey::new("order-42")?;
    let first = lgwks_bot::script::block_on(host.submit(&key, &work, 7u32))?;
    assert!(
        matches!(first, Submission::Executed(_)),
        "a first submission executes"
    );
    let run = first.run_id().ok_or("a submission names a run")?;
    assert_eq!(counter.get(), 1, "the first submission ran the body once");

    // The same key with a different payload is a conflict naming the key, and
    // no body runs for it.
    let conflict = lgwks_bot::script::block_on(host.submit(&key, &work, 8u32));
    assert!(
        matches!(conflict, Err(RequestError::Conflict(_))),
        "a different digest under one key is a typed Conflict: {conflict:?}"
    );
    assert_eq!(
        counter.get(),
        1,
        "a conflicted submission never entered the body"
    );

    // A duplicate identical request reattaches to the recorded report.
    let second = lgwks_bot::script::block_on(host.submit(&key, &work, 7u32))?;
    assert!(
        matches!(second, Submission::Reattached(_)),
        "a repeat under the same key and input reattaches"
    );
    assert_eq!(
        second.run_id(),
        Some(run),
        "one key derives one run on every submission"
    );
    assert_eq!(
        second.report().and_then(|report| report.output().copied()),
        Some(7),
        "the recorded output comes back without the body running"
    );
    assert_eq!(
        counter.get(),
        1,
        "the reattach never entered the body again"
    );

    // Distinct keys are distinct runs, never silently collapsed.
    let other =
        lgwks_bot::script::block_on(host.submit(&RequestKey::new("order-43")?, &work, 7u32))?;
    assert!(
        matches!(other, Submission::Executed(_)),
        "a distinct key executes its own run"
    );
    assert!(
        other.run_id() != Some(run),
        "distinct keys derive distinct runs"
    );
    assert_eq!(counter.get(), 2, "the distinct run entered the body once");
    Ok(())
}
