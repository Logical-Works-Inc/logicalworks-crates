//! The T35 family: the three untrue successes report distinct true outcomes.

use super::*;

// ── T35: three untrue successes, three distinct true outcomes ────────────────

/// The row's three arms report three distinct outcomes, and none of them is
/// success.
#[test]
fn the_three_untrue_successes_report_distinct_outcomes_t35() -> TestResult {
    let surface = surface()?;
    let decoder = decoder();

    // (1) A process exited zero with an invalid result. The exit code is not an
    // answer: the payload that says so is refused, and the run is not Succeeded.
    let zero_exit = decoder.decode(&surface, b"op=read-report\nnote=exit-0", Source::ToolOutput);
    assert!(
        zero_exit.is_admitted(),
        "a well-formed result from an exit-zero process is admitted as a *proposal*: {zero_exit}"
    );
    let plan = zero_exit
        .plan()
        .ok_or("an admitted outcome carries its plan")?;
    assert!(
        !plan.coverage().is_complete(),
        "but its exit code never becomes full coverage"
    );

    // (2) The model says done with no evidence. That is its own outcome, and it
    // is not success.
    let claim = Completion::claim("all done", &["report-9"]);
    let outcome = CompletionOutcome::settle(
        &claim,
        &[],
        Coverage::Partial {
            covered: 1,
            asked: 1,
        },
    );
    assert!(
        matches!(outcome, CompletionOutcome::NotEvidenced { .. }),
        "a done claim with absent evidence is NotEvidenced, not success: {outcome}"
    );
    assert!(
        !outcome.is_admitted(),
        "and it does not read as admitted to a caller checking one field"
    );

    // (3) A successful draft with a failed publish is a third outcome again:
    // the draft exists and the publish did not happen.
    let drafted = CompletionOutcome::settle(
        &Completion::claim("draft written", &["draft-1"]),
        &[String::from("draft-1")],
        Coverage::Partial {
            covered: 1,
            asked: 1,
        },
    );
    assert!(
        matches!(drafted, CompletionOutcome::Incomplete { .. }),
        "a draft that was not published is incomplete coverage, not NotEvidenced: {drafted}"
    );

    // The three are pairwise distinct, which is the row's actual claim.
    let evidenced = format!("{outcome}");
    let incomplete = format!("{drafted}");
    assert_ne!(
        evidenced, incomplete,
        "a lie about evidence and a short run are different facts and are reported differently"
    );
    assert!(
        evidenced.starts_with("not evidenced") && incomplete.starts_with("incomplete"),
        "and each names its own outcome: {evidenced:?} / {incomplete:?}"
    );

    // A claim that is genuinely evidenced *and* complete is the only thing that
    // is admitted as finished.
    let honest = CompletionOutcome::settle(
        &Completion::claim("published", &["report-1"]),
        &[String::from("report-1")],
        Coverage::Complete,
    );
    assert!(
        honest.is_admitted(),
        "a claim whose evidence is present and whose coverage is complete is admitted: {honest}"
    );
    Ok(())
}

/// An abandoned run is admitted as abandoned — visibly not the same thing as a
/// finished one.
#[test]
fn an_abandoned_run_is_not_a_finished_one() -> TestResult {
    let abandoned = Completion::abandon("the upstream is gone");
    let outcome = CompletionOutcome::settle(
        &abandoned,
        &[],
        Coverage::Partial {
            covered: 0,
            asked: 1,
        },
    );
    assert!(outcome.is_admitted(), "a run may say it stopped: {outcome}");
    assert!(
        format!("{outcome}").contains("Abandoned"),
        "and the outcome names the abandonment rather than a completion: {outcome}"
    );
    assert_eq!(
        abandoned.kind(),
        lgwks_bot::proposal::CompletionKind::Abandoned
    );
    Ok(())
}

/// A claim naming more evidence references than the ceiling is refused whole,
/// rather than trimmed to a prefix that would read as complete.
#[test]
fn an_over_long_evidence_claim_is_refused_whole() -> TestResult {
    let named = (0..lgwks_bot::proposal::MAX_EVIDENCE_REFS.saturating_add(1))
        .map(|index| format!("ref-{index}"))
        .collect::<Vec<_>>();
    let claim = Completion::claim(
        "everything",
        &named.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    assert!(
        matches!(claim.admit(&[]), Err(Refusal::Limit { .. })),
        "a claim past the evidence ceiling is refused by name rather than trimmed"
    );
    Ok(())
}
