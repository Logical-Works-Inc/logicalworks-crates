//! Bounded capture, and a deadline that stops a whole process group.

use std::time::Duration;

use crate::scripted_gh::{Scenario, ScriptedGh};

use super::{HEAD, LossyQuery, TestResult, gh_for};

// ── Bounded capture ─────────────────────────────────────────────────────────

#[test]
fn a_chatty_client_is_truncated_at_its_ceiling_and_never_decoded() -> TestResult {
    let script = ScriptedGh::install("flood", HEAD)?;
    // 200 lines of 49 bytes each is about 9,800 bytes, and the ceiling is 1,024.
    // A read that returns more than the ceiling must not be parsed: a partial
    // JSON document decoded into a partial value is the defect.
    script.configure(Scenario::new(HEAD).floods(200))?;
    let gh = gh_for(&script, 1024, Duration::from_secs(10))?;

    let outcome =
        lgwks_bot::Runtime::new()?.block_on(gh.query_lossy("repos/acme/widgets/pulls/7"))?;

    assert!(
        outcome.stdout_truncated(),
        "a child that wrote past the ceiling is reported as truncated: {outcome:?}"
    );
    assert!(
        outcome.stdout_total_bytes() > 1024,
        "and the exact total is reported, so a reader knows how much was not retained: {}",
        outcome.stdout_total_bytes()
    );
    assert!(
        outcome.stdout().len() <= 1024,
        "the retained bytes stop at the ceiling: {}",
        outcome.stdout().len()
    );
    assert_eq!(
        outcome.exit_code(),
        Some(0),
        "the child succeeded; truncation is the adapter's bound, not its failure"
    );

    // The parse is refused rather than attempted on a partial document.
    let decoded = outcome.parse_json::<lgwks_bot::domain::gh::PrSnapshot>();
    assert!(
        matches!(
            decoded,
            Err(lgwks_bot::domain::gh::GhError::TruncatedResponse { .. })
        ),
        "a truncated answer is refused, never decoded into a partial value: {decoded:?}"
    );
    Ok(())
}

#[test]
fn a_response_within_the_ceiling_is_parsed() -> TestResult {
    let script = ScriptedGh::install("small", HEAD)?;
    script.configure(Scenario::new(HEAD))?;
    let gh = gh_for(&script, 64 * 1024, Duration::from_secs(10))?;

    let outcome =
        lgwks_bot::Runtime::new()?.block_on(gh.query_lossy("repos/acme/widgets/pulls/7"))?;
    assert!(
        !outcome.stdout_truncated(),
        "a small answer is not truncated"
    );
    let snapshot = outcome.parse_json::<lgwks_bot::domain::gh::PrSnapshot>()?;
    assert_eq!(
        snapshot.head_sha(),
        HEAD,
        "an answer within the ceiling is decoded, not refused"
    );
    Ok(())
}

// ── The deadline stops a whole process group ────────────────────────────────

#[test]
fn a_hanging_client_is_stopped_as_a_whole_group_and_reports_no_exit_code() -> TestResult {
    let script = ScriptedGh::install("hang", HEAD)?;
    // The fixture spawns a grandchild and then sleeps, so the deadline has a
    // process *group* to reap rather than one child it could trivially kill.
    script.configure(Scenario::new(HEAD).hangs_for(30))?;
    let gh = gh_for(&script, 64 * 1024, Duration::from_millis(400))?;

    let started = std::time::Instant::now();
    let outcome =
        lgwks_bot::Runtime::new()?.block_on(gh.query_lossy("repos/acme/widgets/pulls/7"))?;
    let elapsed = started.elapsed();

    assert!(
        outcome.deadline_fired(),
        "a client that hangs is stopped by its deadline: {outcome:?}"
    );
    assert_eq!(
        outcome.exit_code(),
        None,
        "a deadline stop is not an exit code: the child did not choose to exit"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the deadline is honoured rather than the client being waited out: {elapsed:?}"
    );
    Ok(())
}
