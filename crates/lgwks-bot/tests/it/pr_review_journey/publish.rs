//! The publish path: a review lands at the pinned head and is verified by a
//! separate read, and a failed analysis publishes nothing.

use lgwks_bot::domain::gh::CommitId;

use crate::scripted_gh::ScriptedGh;

use super::{HEAD, TestResult, failing_review_run, gh_for, payload_of, run_succeeding};

#[test]
fn a_review_is_published_at_the_pinned_head_and_verified_by_a_separate_read() -> TestResult {
    let scripted = ScriptedGh::install("publish", HEAD)?;
    let report = run_succeeding(
        &scripted,
        "a published, verified review is a successful run",
    )?;
    let outcome = report
        .output()
        .ok_or("a successful run carries its outcome")?;
    assert!(
        outcome.is_published(),
        "the happy path publishes and verifies: {outcome:?}"
    );
    assert_eq!(
        outcome.commit_id().map(CommitId::as_str),
        Some(HEAD),
        "the review names the commit that was read"
    );

    // The create carried the pinned commit, and the read-back was a separate
    // invocation: three reads/writes total — snapshot, freshness, create,
    // verify — with exactly one POST.
    assert_eq!(
        scripted.creates()?,
        1,
        "exactly one create, or the run posted the review twice"
    );
    assert!(
        scripted.reads_of("/pulls/7/reviews")? >= 1,
        "the publication is verified by a read-back, not by the create's exit code"
    );
    let payload =
        payload_of(&scripted, 0)?.ok_or("the create must name the payload file it was given")?;
    assert!(
        payload.contains(HEAD),
        "the create payload must carry the reviewed commit id: {payload}"
    );
    assert!(
        payload.contains("\"commit_id\""),
        "the reviewed commit is a named field of the payload, never omitted: {payload}"
    );
    Ok(())
}

#[test]
fn a_failed_analysis_publishes_nothing() -> TestResult {
    let scripted = ScriptedGh::install("script-fails", HEAD)?;
    let run = failing_review_run()?;
    let gh = gh_for(&scripted)?;

    let report = run.host.block_on(&run.task, (gh, run.request))?;
    assert_eq!(
        report.disposition().label(),
        "Failed",
        "a failed analysis is a failed run, not an empty successful review"
    );
    assert_eq!(
        scripted.creates()?,
        0,
        "an analysis that produced no body must not create a review"
    );
    Ok(())
}
