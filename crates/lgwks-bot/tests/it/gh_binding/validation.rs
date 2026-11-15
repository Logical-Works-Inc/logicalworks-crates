//! What the binding refuses before anything runs.

use lgwks_bot::domain::gh::{CommitId, Gh, GhError, Repository};

use super::{HEAD, TestResult};

// ── Validation refusals ─────────────────────────────────────────────────────

#[test]
fn a_repository_reference_that_is_not_one_is_refused_before_anything_runs() -> TestResult {
    for bad in [
        "",
        "no-slash",
        "too/many/slashes",
        "/leading",
        "trailing/",
        "owner/has space",
        "owner/re po",
    ] {
        assert!(
            Repository::new(bad).is_err(),
            "{bad:?} is not an owner/repo reference and must be refused"
        );
    }
    let ok = Repository::new("acme/widgets.git")?;
    assert_eq!(ok.as_str(), "acme/widgets.git");
    assert!(
        Repository::new("a/b").is_ok(),
        "control: a minimal valid reference is accepted"
    );
    Ok(())
}

#[test]
fn a_commit_id_that_is_not_one_is_refused() -> TestResult {
    for bad in [
        "",
        "abc",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "111111111111111111111111111111111111111G",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        assert!(
            CommitId::new(bad).is_err(),
            "{bad:?} is not a full lowercase commit id and must be refused"
        );
    }
    assert_eq!(
        CommitId::new(HEAD)?.as_str(),
        HEAD,
        "control: a full lowercase sha is accepted"
    );
    Ok(())
}

#[test]
fn a_review_event_github_does_not_accept_is_refused() -> TestResult {
    let subject = CommitId::new(HEAD)?;
    for event in ["", "MERGE", "comment", "APPROVED"] {
        assert!(
            lgwks_bot::domain::gh::ReviewPayload::new(&subject, event, "body", "marker").is_err(),
            "{event:?} is not a review event GitHub accepts"
        );
    }
    for event in lgwks_bot::domain::gh::ReviewPayload::EVENTS {
        let payload = lgwks_bot::domain::gh::ReviewPayload::new(&subject, event, "body", "marker")?;
        assert_eq!(
            payload.commit_id(),
            HEAD,
            "the reviewed commit is a field of the payload, never omitted"
        );
    }
    Ok(())
}

#[test]
fn a_build_without_a_runner_refuses_rather_than_reporting_an_empty_answer() {
    // The vocabulary is the point: a binding that cannot reach GitHub must say
    // so, because a caller handed "no reviews" would conclude GitHub has none.
    let error = GhError::NoRunner;
    assert!(
        error.to_string().contains("process runner"),
        "the refusal names the missing capability: {error}"
    );
    assert!(
        !error.is_read_only_retryable(),
        "and it is not retryable, because retrying cannot conjure a runner"
    );
}

// ── The argument vector ─────────────────────────────────────────────────────

#[test]
fn the_argument_vector_is_a_vector_and_names_the_method_and_path() -> TestResult {
    let gh = Gh::new(Repository::new("acme/widgets")?);
    let args = gh.args_for(&["--method", "GET", "repos/acme/widgets/pulls/7"]);

    assert_eq!(
        args,
        vec!["api", "--method", "GET", "repos/acme/widgets/pulls/7"],
        "the client's own arguments are passed through unchanged, with no shell \
         between them and the process"
    );
    assert!(
        gh.args_for(&["--method", "POST"])
            .iter()
            .all(|arg| !arg.contains(';') && !arg.contains('|')),
        "no shell metacharacter is introduced by the binding itself"
    );
    Ok(())
}
