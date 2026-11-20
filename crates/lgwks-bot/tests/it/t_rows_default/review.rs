//! The review-adapter rows: T31 sandbox, T32 moved head, T33 reconciliation, T34 read-back.

use super::*;

// ── T31 ─────────────────────────────────────────────────────────────────────

/// T31, sandbox half: without the `process` feature every adapter call is a
/// typed `NoRunner` — a binding that cannot run a client says so rather than
/// pretending it read GitHub — and repository and commit subjects validate at
/// construction in every build.
///
/// With `process` the adapter binds a real runner instead, so the `NoRunner`
/// arm below compiles only without it; the subject validation runs in both.
/// Diff ceilings, pagination and the fake-`gh` journeys stay with the
/// `process` tests the map names.
#[test]
fn without_a_runner_nothing_executes_and_subjects_stay_typed_t31() -> TestResult {
    let repository = Repository::new("acme/web")?;
    assert_eq!(repository.as_str(), "acme/web");
    assert!(
        Repository::new("").is_err(),
        "an empty repository reference is refused at construction"
    );
    assert!(
        Repository::new("not a repo !!!").is_err(),
        "a malformed repository reference is refused at construction"
    );
    let pull = PullRequest::new(repository, 4821);
    assert_eq!(
        pull.number(),
        4821,
        "the subject pins repository and number"
    );

    // Without a supervised runner the adapter refuses typed rather than
    // returning an empty answer. This arm exists only where there is no
    // runner to bind: with `process` the same call runs a real client.
    #[cfg(not(feature = "process"))]
    {
        let adapter = Gh::new(
            PullRequest::new(Repository::new("acme/web")?, 4821)
                .repository()
                .clone(),
        );
        let snapshot = lgwks_bot::script::block_on(adapter.snapshot(&pull));
        assert!(
            matches!(snapshot, Err(GhError::NoRunner)),
            "without a supervised runner the adapter refuses typed rather than returning an empty answer: {snapshot:?}"
        );
    }
    Ok(())
}

// ── T32 ─────────────────────────────────────────────────────────────────────

/// T32, detection half: two snapshots pinning different heads decode to
/// different pinned commits with both shas named, so a move reads as
/// reviewed-A/current-B rather than as evidence relabelled to B.
///
/// The refusal itself stays with the `process` journey the map names.
#[test]
fn a_moved_head_is_detectable_as_reviewed_a_current_b_t32() -> TestResult {
    let read: PrSnapshot = lgwks_std::json::from_str(
        r#"{"number":4821,"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"base":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#,
    )?;
    let current: PrSnapshot = lgwks_std::json::from_str(
        r#"{"number":4821,"head":{"sha":"cccccccccccccccccccccccccccccccccccccccc"},"base":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#,
    )?;
    assert_eq!(
        read.head_sha(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "the analysis-time read pins head A"
    );
    assert_eq!(
        current.head_sha(),
        "cccccccccccccccccccccccccccccccccccccccc",
        "the current read pins head B"
    );
    assert!(
        read.head_sha() != current.head_sha(),
        "a head that advanced between analysis and publication is detectable from the two pinned commits"
    );
    assert_eq!(
        read.base_sha(),
        current.base_sha(),
        "the base both snapshots agree on is still named"
    );
    let same: PrSnapshot = lgwks_std::json::from_str(
        r#"{"number":4821,"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"base":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#,
    )?;
    assert_eq!(
        read.head_sha(),
        same.head_sha(),
        "an unmoved head pins the same commit on both reads"
    );
    Ok(())
}

// ── T33 ─────────────────────────────────────────────────────────────────────

/// T33, reconciliation half: an exact read-back verifies, a body mismatch does
/// not, and a partial comment count verifies only except-comments. No adapter
/// is called by any arm: the decision is pure.
#[test]
fn reconciliation_verifies_the_exact_review_or_stays_unknown_t33() -> TestResult {
    let subject = CommitId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
    let intended = ReviewPayload::new(&subject, "COMMENT", "looks good", "t33-marker")?
        .with_comments(vec![ReviewComment::new("src/lib.rs", 10, "nit: rename")]);
    let body = intended.body().to_owned();
    // The body travels inside JSON here, so its newlines are escaped: a raw
    // control character would be a malformed fixture, not a mismatch.
    let json_body = body.replace('\n', "\\n");

    // The exact read-back: same subject, body (marker included), state and
    // comment count.
    let exact: ReviewRecord = lgwks_std::json::from_str(&format!(
        r#"{{"id":99,"commit_id":"{}","state":"COMMENTED","body":"{}","comment_count":1}}"#,
        subject.as_str(),
        json_body
    ))?;
    assert!(
        exact.matches(&intended),
        "the exact read-back verifies: id 99 is the review the payload asked for"
    );
    assert_eq!(exact.id(), 99, "verification names the exact review id");

    // A different body is a different review, even with the right marker.
    let wrong_body = ReviewRecord::new(99, subject.as_str(), "COMMENTED", "something else");
    assert!(
        !wrong_body.matches(&intended),
        "a matching marker with a different body is a different review"
    );
    assert!(
        !wrong_body.matches_except_comments(&intended),
        "and it does not verify except-comments either"
    );

    // A partial submission verifies except-comments but never whole.
    let partial = ReviewRecord::new(99, subject.as_str(), "COMMENTED", &body);
    assert!(
        !partial.matches(&intended),
        "a review whose comments landed only in part is not the whole one"
    );
    assert!(
        partial.matches_except_comments(&intended),
        "but its subject, body and state verify, so reconciliation reports partial rather than unknown"
    );

    // Nothing observed at all stays unknown: no match, no post.
    let stranger = ReviewRecord::new(100, subject.as_str(), "COMMENTED", "unrelated review");
    assert!(
        !stranger.matches(&intended),
        "an unrelated review never verifies against this payload"
    );
    Ok(())
}

// ── T34 ─────────────────────────────────────────────────────────────────────

/// T34, read-back half: a read-back record proves its exact id, commit, state,
/// body and comment count. The lost-permission journey stays with the
/// `process` tests the map names.
#[test]
fn a_read_back_review_proves_its_exact_identity_t34() {
    let record = ReviewRecord::new(
        4821,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "CHANGES_REQUESTED",
        "needs a test for the moved head",
    );
    assert_eq!(record.id(), 4821, "the exact review id");
    assert_eq!(
        record.commit_id(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        "the exact commit the review is about"
    );
    assert_eq!(
        record.state(),
        "CHANGES_REQUESTED",
        "the exact reported state"
    );
    assert_eq!(
        record.body(),
        Some("needs a test for the moved head"),
        "the exact payload"
    );
    assert_eq!(
        record.applied_comments(),
        0,
        "no comment count reported means none established, never all landed"
    );
    assert!(!record.is_pending(), "a decided review is not pending");

    let pending = ReviewRecord::new(
        4822,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "PENDING",
        "draft",
    );
    assert!(pending.is_pending(), "a pending review reads as pending");
    assert!(
        pending.id() != record.id(),
        "two records keep their distinct identities"
    );
}
