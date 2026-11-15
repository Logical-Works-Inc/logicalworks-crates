//! Partial submissions (T33) and a lost read permission (T34).

use lgwks_bot::review::ReviewOutcome;

use crate::scripted_gh::{Scenario, ScriptedGh};

use super::{HEAD, TestResult, gh_for, review_run_with_comments, run_passing};

/// A lost response that landed as an unsubmitted draft is reported as a draft,
/// never as a publication and never re-posted.
#[test]
fn a_pending_draft_is_reconciled_as_a_draft_and_never_reposted() -> TestResult {
    let scripted = ScriptedGh::install("pending-draft", HEAD)?;
    scripted.configure(Scenario::new(HEAD).records_pending_draft())?;
    let report = run_passing(&scripted)?;

    let outcome = report
        .output()
        .ok_or("a pending draft is a reported outcome")?;
    assert!(
        outcome.is_pending(),
        "a lost response onto an unsubmitted draft is Pending: {outcome:?}"
    );
    assert!(
        !outcome.is_published(),
        "an unsubmitted draft is never a published review"
    );
    assert!(
        outcome.review_id().is_some(),
        "the draft's id is retained so recovery can target it: {outcome:?}"
    );
    assert_eq!(
        scripted.creates()?,
        1,
        "the draft was created once; reconciliation never posts a second review"
    );
    Ok(())
}

/// A lost response whose comments landed only in part is reported as partial,
/// with both counts, and never re-posted.
#[test]
fn a_partial_submission_is_reconciled_as_partial_and_never_reposted() -> TestResult {
    let scripted = ScriptedGh::install("partial-comments", HEAD)?;
    // The request intends two comments; the receiver lands one.
    scripted.configure(Scenario::new(HEAD).records_partial_comments(1))?;

    let run = review_run_with_comments(2)?;
    let report = run
        .host
        .block_on(&run.task, (gh_for(&scripted)?, run.request))?;

    let outcome = report
        .output()
        .ok_or("a partial submission is a reported outcome")?;
    match *outcome {
        ReviewOutcome::Partial {
            applied, intended, ..
        } => {
            assert_eq!(
                (applied, intended),
                (1, 2),
                "the partial state names what landed and what was intended: {outcome:?}"
            );
        }
        ref other => {
            return Err(format!("a partial submission must be Partial, not {other:?}").into());
        }
    }
    assert_eq!(
        scripted.creates()?,
        1,
        "the partial review was created once; recovery targets only what remains"
    );
    Ok(())
}

// ── Lost read permission (T34) ──────────────────────────────────────────────

/// A publication whose read-back lost permission is Unverified, with the
/// applied review id retained — never a clean failure and never a second write.
#[test]
fn a_lost_read_permission_reports_unverified_and_retains_the_review_id_t34() -> TestResult {
    let scripted = ScriptedGh::install("read-denied", HEAD)?;
    // The create is accepted and returns an id; the review read is then denied
    // with a `403`, which the adapter reports as a typed permission failure.
    scripted.configure(Scenario::new(HEAD).deny_reads())?;
    let report = run_passing(&scripted)?;

    let outcome = report
        .output()
        .ok_or("an unverified publication is a reported outcome")?;
    match *outcome {
        ReviewOutcome::Unverified {
            review_id,
            ref commit_id,
            ref reason,
        } => {
            assert!(
                review_id >= 9_001,
                "the applied review id is retained as evidence: {review_id}"
            );
            assert_eq!(
                commit_id.as_str(),
                HEAD,
                "and the reviewed commit is retained"
            );
            assert!(
                reason.contains("permission"),
                "the reason names what was lost: {reason}"
            );
        }
        ref other => {
            return Err(format!("lost read permission must be Unverified, not {other:?}").into());
        }
    }
    assert_eq!(
        scripted.creates()?,
        1,
        "a lost read permission is never resolved by a second create"
    );
    Ok(())
}
