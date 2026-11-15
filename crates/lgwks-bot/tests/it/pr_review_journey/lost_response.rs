//! The lost response: reconciled by reading back, never re-posted, and a
//! client that never starts is a definite non-effect.

use std::time::Duration;

use lgwks_bot::domain::gh::{Gh, Repository};

use crate::scripted_gh::{Scenario, ScriptedGh};

use super::{HEAD, TestResult, error_text, review_run, run_passing, run_succeeding};

#[test]
fn a_lost_response_is_reconciled_by_reading_back_and_never_reposted_t33() -> TestResult {
    let scripted = ScriptedGh::install("lost", HEAD)?;
    // The create applies the review and then drops its response, so the effect
    // landed and the caller cannot learn that from the create call.
    scripted.configure(
        Scenario::new(HEAD)
            .accept_then_drop()
            .created_body("two findings"),
    )?;
    let report = run_succeeding(
        &scripted,
        "a lost response that the read-back reconciles is a published review",
    )?;
    let outcome = report
        .output()
        .ok_or("a successful run carries its outcome")?;
    assert!(
        outcome.is_published(),
        "the read-back found the review the lost response was about: {outcome:?}"
    );
    assert_eq!(
        scripted.creates()?,
        1,
        "the whole point: a lost response must not become a second create"
    );
    Ok(())
}

#[test]
fn a_loss_that_cannot_be_reconciled_stays_unknown_and_still_does_not_repost() -> TestResult {
    let scripted = ScriptedGh::install("unreconcilable", HEAD)?;
    // The create fails without applying, and the read-back is refused, so
    // whether anything landed cannot be established from either direction.
    scripted.configure(Scenario::new(HEAD).refuse_creates().fail_reads(1))?;
    let report = run_passing(&scripted)?;
    // The read-back itself failed, so the outcome is Unknown rather than a
    // clean failure — a caller must be able to tell "nothing happened" from
    // "I cannot tell".
    let outcome = report
        .output()
        .ok_or("an unknown outcome is a value, not a run failure")?;
    assert!(
        outcome.is_unknown(),
        "an unreadable read-back after a possible write is unknown: {outcome:?}"
    );
    assert_eq!(
        scripted.creates()?,
        1,
        "the run issued one create and reconciled; it must not issue a second"
    );
    Ok(())
}

#[test]
fn a_non_zero_exit_is_not_a_published_review() -> TestResult {
    let scripted = ScriptedGh::install("nonzero", HEAD)?;
    // The create is refused outright and nothing is applied, so the read-back
    // finds no review: the run must not report a publication.
    scripted.configure(Scenario::new(HEAD).refuse_creates())?;
    let report = run_passing(&scripted)?;
    let outcome = report
        .output()
        .ok_or("a client that exited non-zero still reports an outcome")?;
    assert!(
        !outcome.is_published(),
        "a client that exited non-zero never reports a publication: {outcome:?}"
    );
    assert_eq!(
        scripted.creates()?,
        1,
        "the failed create was reconciled by a read, not repeated"
    );
    assert!(
        scripted.reads_of("/pulls/7/reviews")? >= 1,
        "and the reconciliation was a read, which is the only safe next move \
         after an exit code that cannot prove a non-effect"
    );
    Ok(())
}

#[test]
fn a_client_that_never_starts_is_a_definite_non_effect_and_is_not_reconciled() -> TestResult {
    // No scripted client is installed: the binding names a program that does not
    // exist, so the platform refuses the fork and nothing can have reached
    // GitHub. That is the one publish failure that is *certainly* a non-effect,
    // and it is reported as a failure rather than as an unknown — the
    // classification is a real distinction, not a formality.
    let run = review_run()?;
    let gh = Gh::new(Repository::new("acme/widgets")?)
        .program("lgwks-no-such-github-client-on-this-path")
        .deadline(Some(Duration::from_secs(5)));

    let report = run.host.block_on(&run.task, (gh, run.request))?;
    assert_eq!(
        report.disposition().label(),
        "Failed",
        "a client that never started is a clean failure, not an unknown: {}",
        error_text(&report)
    );
    assert!(
        report.output().is_none(),
        "and it reports no outcome at all, because there is no publication to \
         describe and no effect to leave unknown"
    );
    Ok(())
}
