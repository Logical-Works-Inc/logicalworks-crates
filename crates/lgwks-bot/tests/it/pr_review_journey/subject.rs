//! The subject of a review: a moved head, isolation between identities, the
//! adapter's capability, and coverage (diff, rename, sandbox; T31).

use lgwks_bot::domain::gh::{CommitId, Gh, PullRequest, Repository};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::KernelError;
use lgwks_bot::review::{ReviewOutcome, ReviewRequest};
use lgwks_bot::script::Scope;
use lgwks_bot::task::task;

use crate::scripted_gh::{Scenario, ScriptedGh};

use super::{
    HEAD, MOVED_HEAD, TestResult, body_script, gh_for, gh_with_capture, host, payload_of, pull,
    review_run, run_passing,
};

// ── The moved subject ───────────────────────────────────────────────────────

#[test]
fn a_moved_head_is_a_typed_refusal_and_publishes_nothing_t32() -> TestResult {
    let scripted = ScriptedGh::install("moved", HEAD)?;
    // `head_after_first` makes the script report HEAD on its first read and
    // MOVED_HEAD on every read after it — a push landing between the snapshot
    // and the freshness check, which is the race the refusal exists for.
    scripted.configure(Scenario::new(HEAD).head_moves_to(MOVED_HEAD))?;
    let report = run_passing(&scripted)?;
    let outcome = report
        .output()
        .ok_or("a moved head is a reported outcome, not a run failure")?;
    match *outcome {
        ReviewOutcome::TargetMoved {
            ref reviewed,
            ref current,
        } => {
            assert_eq!(
                reviewed.as_str(),
                HEAD,
                "the refusal names the commit that was actually reviewed"
            );
            assert_eq!(
                current, MOVED_HEAD,
                "and the commit the pull request now points at, so the caller \
                 can decide whether to re-analyse"
            );
        }
        ref other => {
            return Err(format!("a moved head must be TargetMoved, not {other:?}").into());
        }
    }
    assert_eq!(
        scripted.creates()?,
        0,
        "a review of code nobody read must never be published"
    );
    Ok(())
}

// ── Isolation ───────────────────────────────────────────────────────────────

#[test]
fn two_identities_on_one_repository_stay_isolated() -> TestResult {
    let scripted = ScriptedGh::install("isolation", HEAD)?;
    let host = host()?;

    // Two reviews of the same repository with distinct bodies. Each create must
    // carry only its own payload, and each verification must match only its own
    // review — the fixture answers reads with the full list, so a run that
    // matched the *other* body's review would report the wrong review id.
    scripted.configure(Scenario::new(HEAD))?;

    let build = |_body: &'static str| {
        task(
            "review-pr",
            move |scope: Scope, (gh, pull, body): (Gh, PullRequest, &'static str)| async move {
                let request = ReviewRequest::new(pull, "COMMENT", body).with_marker(body);
                lgwks_bot::review::review_pr(scope, gh, request, body_script(body)).await
            },
        )
    };

    let first = build("first identity")?;
    let second = build("second identity")?;

    let first_report = host.block_on(&first, (gh_for(&scripted)?, pull()?, "first identity"))?;
    let second_report = host.block_on(&second, (gh_for(&scripted)?, pull()?, "second identity"))?;
    for report in [&first_report, &second_report] {
        assert_eq!(
            report.disposition().label(),
            "Succeeded",
            "both identities publish independently"
        );
    }
    assert_eq!(
        scripted.creates()?,
        2,
        "two identities on one repository are two reviews, not one and not three"
    );
    let first_payload = payload_of(&scripted, 0)?.ok_or("the first create's payload")?;
    let second_payload = payload_of(&scripted, 1)?.ok_or("the second create's payload")?;
    assert!(
        first_payload.contains("first identity"),
        "the first create carries only its own body: {first_payload}"
    );
    assert!(
        second_payload.contains("second identity"),
        "the second create carries only its own body: {second_payload}"
    );
    assert!(
        !first_payload.contains("second identity"),
        "a tenant's payload must not carry another's body"
    );
    Ok(())
}

// ── Capability ──────────────────────────────────────────────────────────────

#[test]
fn the_adapter_refuses_without_its_capability_before_starting_anything() -> TestResult {
    // No scripted client is installed: the refusal happens before the fork, so
    // there is nothing to intercept and nothing should be started.
    let query = lgwks_bot::domain::gh::GhQuery::new(
        Gh::new(Repository::new("acme/widgets")?),
        vec![
            String::from("--method"),
            String::from("GET"),
            String::from("rate_limit"),
        ],
    );
    // A grant of the wrong capability: notification delivery, not process
    // control or the network. The adapter requires both of the latter, so the
    // check names every one of them that is missing.
    let auth = lgwks_bot::gate::GrantSet::empty()
        .grant(lgwks_bot::cap::Cap::notify())
        .issue(&[])?;
    let denied = lgwks_bot::rt::runtime::Runtime::new()?
        .block_on(async { lgwks_bot::verb::Query::query(&query, (auth, &())).await })
        .err()
        .ok_or("a query without bot.sys and bot.net must be denied")?;
    match denied {
        EcsError::Kernel(KernelError::CapabilityDenied { deficit }) => {
            let missing: Vec<&str> = deficit
                .shortages()
                .map(|shortage| shortage.required().as_str())
                .collect();
            assert_eq!(
                missing,
                vec![lgwks_bot::cap::Cap::SYS, lgwks_bot::cap::Cap::NET],
                "the denial names every capability the GitHub binding needs"
            );
        }
        other => return Err(format!("expected a capability denial, got {other:?}").into()),
    }
    Ok(())
}

// ── Subject coverage: diff, rename and sandbox (T31) ────────────────────────

/// An unavailable diff is an *incomplete coverage*: nothing is published.
///
/// The server declining to render the diff is a coverage decision, not a
/// transport failure: the code that was read is only part of what the pull
/// request changes, and a review against it would claim a scope nobody read.
#[test]
fn an_unavailable_diff_is_an_incomplete_coverage_and_publishes_nothing() -> TestResult {
    let scripted = ScriptedGh::install("diff-unavailable", HEAD)?;
    scripted.configure(Scenario::new(HEAD).diff_unavailable())?;
    let report = run_passing(&scripted)?;

    let outcome = report
        .output()
        .ok_or("an incomplete coverage is a reported outcome, not a run failure")?;
    assert!(
        outcome.is_incomplete(),
        "an unavailable diff is an incomplete coverage: {outcome:?}"
    );
    assert!(
        !outcome.is_published(),
        "an incomplete coverage must never read as a clean review"
    );
    assert_eq!(
        outcome.commit_id().map(CommitId::as_str),
        Some(HEAD),
        "the pinned commit is still named, so the caller can re-run at it"
    );
    assert_eq!(
        scripted.creates()?,
        0,
        "nothing is published when the coverage is incomplete"
    );
    Ok(())
}

/// Run the journey with a raised capture ceiling and return its outcome.
///
/// The two diff-ceiling probes need an answer larger than the default capture,
/// so the envelope is raised: the refusal under test is the *diff* ceiling, not
/// the capture bound. One helper rather than a copied block, so the two probes
/// cannot drift apart.
fn run_with_large_capture(
    scripted: &ScriptedGh,
) -> Result<ReviewOutcome, Box<dyn std::error::Error>> {
    let run = review_run()?;
    let report = run.host.block_on(
        &run.task,
        (gh_with_capture(scripted, 4 * 1024 * 1024)?, run.request),
    )?;
    report
        .output()
        .cloned()
        .ok_or_else(|| "a coverage decision is a reported outcome".into())
}

/// A diff inventory past its file-count ceiling is an incomplete coverage.
///
/// The ceiling is a *typed refusal* rather than a truncated list: a prefix of
/// the inventory that decoded cleanly is indistinguishable from the whole diff.
#[test]
fn a_diff_past_the_file_ceiling_is_an_incomplete_coverage() -> TestResult {
    let scripted = ScriptedGh::install("diff-over-files", HEAD)?;
    let over = u32::try_from(lgwks_bot::domain::gh::MAX_DIFF_FILES_PER_PULL)
        .map_err(|_| "the file ceiling must fit the fixture's counter")?
        .saturating_add(1);
    scripted.configure(Scenario::new(HEAD).with_filler_files(over))?;

    let outcome = run_with_large_capture(&scripted)?;
    assert!(
        outcome.is_incomplete(),
        "a diff past its file ceiling is an incomplete coverage: {outcome:?}"
    );
    assert_eq!(scripted.creates()?, 0, "nothing is published");
    Ok(())
}

/// A diff past its byte ceiling is an incomplete coverage, on its own axis.
#[test]
fn a_diff_past_the_byte_ceiling_is_an_incomplete_coverage() -> TestResult {
    let scripted = ScriptedGh::install("diff-over-bytes", HEAD)?;
    let over = u32::try_from(lgwks_bot::domain::gh::MAX_DIFF_BYTES)
        .map_err(|_| "the byte ceiling must fit the fixture's counter")?
        .saturating_add(64);
    scripted.configure(Scenario::new(HEAD).with_diff_bytes(over))?;

    let outcome = run_with_large_capture(&scripted)?;
    assert!(
        outcome.is_incomplete(),
        "a diff past its byte ceiling is an incomplete coverage: {outcome:?}"
    );
    assert_eq!(scripted.creates()?, 0, "nothing is published");
    Ok(())
}

/// A renamed repository is a typed refusal naming both names, never a silent
/// re-point at the canonical repository.
#[test]
fn a_renamed_repository_is_refused_and_never_silently_re_pointed() -> TestResult {
    let scripted = ScriptedGh::install("moved-repo", HEAD)?;
    scripted.configure(Scenario::new(HEAD).repository_moved())?;
    let report = run_passing(&scripted)?;

    let outcome = report
        .output()
        .ok_or("a moved repository is a reported outcome")?;
    match *outcome {
        ReviewOutcome::Refused { ref reason } => {
            assert!(
                reason.contains("acme/widgets"),
                "the refusal names the repository the caller requested: {reason}"
            );
            assert!(
                reason.contains("acme/newrepo"),
                "and the canonical repository the answer named: {reason}"
            );
        }
        ref other => {
            return Err(format!("a renamed repository must be Refused, not {other:?}").into());
        }
    }
    assert_eq!(
        scripted.creates()?,
        0,
        "a subject identity that moved is never reviewed under a name nobody asked for"
    );
    Ok(())
}

/// The pull request's build script is inspected, never executed (INV-BOT-21).
///
/// The changed-file inventory names a `build.rs` whose patch text would remove
/// the file named by `LGWKS_TEST_MARKER` if anything ran it. The journey reads
/// the inventory as data, so the marker is untouched; a run that executed the
/// patch would trip the same oracle.
#[test]
fn an_untrusted_build_script_in_the_diff_is_never_executed_t31() -> TestResult {
    let scripted = ScriptedGh::install("build-script", HEAD)?;
    scripted.configure(Scenario::new(HEAD).hostile_build_script())?;
    // The marker lives inside the fixture's own directory and is named in the
    // child's environment, so a shell that ran the patch would find and remove
    // it; the fixture removes it at the end of the test regardless.
    let marker = scripted.dir().join("LGWKS_MARKER");
    std::fs::write(&marker, b"present")?;

    let run = review_run()?;
    let gh = gh_with_capture(&scripted, 64 * 1024)?.env("LGWKS_TEST_MARKER", marker.as_os_str());
    let report = run.host.block_on(&run.task, (gh, run.request))?;

    assert!(
        marker.exists(),
        "the review path must never execute the pull request's build script"
    );
    let outcome = report
        .output()
        .ok_or("a build script in the inventory does not stop the review")?;
    assert!(
        outcome.is_published(),
        "the inventory is read as data, so a hostile file still publishes: {outcome:?}"
    );
    Ok(())
}
