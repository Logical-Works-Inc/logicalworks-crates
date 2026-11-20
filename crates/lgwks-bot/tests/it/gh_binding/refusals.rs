//! Typed refusals the review journey depends on (T31, T34), and a refused
//! credential reaching the verb path as its repair.

use std::time::Duration;

use lgwks_bot::domain::gh::{GhError, PullRequest, Repository};

use crate::scripted_gh::{Scenario, ScriptedGh};

use super::{HEAD, TestResult, gh_for};

/// The pull request every refusal test reads.
fn pull() -> Result<PullRequest, Box<dyn std::error::Error>> {
    Ok(PullRequest::new(Repository::new("acme/widgets")?, 7))
}

/// A renamed repository is a typed move naming both names, never a silent
/// re-point at the canonical one.
#[test]
fn a_renamed_repository_is_a_typed_move_naming_both_names() -> TestResult {
    let script = ScriptedGh::install("moved-adapter", HEAD)?;
    script.configure(Scenario::new(HEAD).repository_moved())?;
    let gh = script.binding("acme/widgets", 64 * 1024)?;

    let error = lgwks_bot::rt::runtime::Runtime::new()?
        .block_on(gh.snapshot(&pull()?))
        .err()
        .ok_or("a renamed repository must be refused, not silently followed")?;
    match error {
        GhError::MovedRepository {
            ref requested,
            ref canonical,
        } => {
            assert_eq!(
                requested, "acme/widgets",
                "the move names the repository the caller requested"
            );
            assert_eq!(
                canonical, "acme/newrepo",
                "and the canonical repository the answer named"
            );
        }
        other => {
            return Err(format!("expected a typed move, got {other:?} ({other})").into());
        }
    }
    Ok(())
}

/// A `403` from the client is a typed permission refusal, not a transport
/// failure, and retrying it cannot help.
#[test]
fn a_permission_refusal_is_a_typed_unauthorized_not_a_transport_failure() -> TestResult {
    let script = ScriptedGh::install("denied-adapter", HEAD)?;
    script.configure(Scenario::new(HEAD).deny_reads())?;
    let gh = script.binding("acme/widgets", 64 * 1024)?;

    let error = lgwks_bot::rt::runtime::Runtime::new()?
        .block_on(gh.read_reviews(&pull()?))
        .err()
        .ok_or("a denied read must be refused")?;
    assert!(
        !error.is_read_only_retryable(),
        "a permission refusal is not fixed by retrying: {error}"
    );
    match error {
        GhError::Unauthorized { status, .. } => assert_eq!(
            status, 403,
            "the refusal carries the status the client named: {status}"
        ),
        other => {
            return Err(format!("expected Unauthorized, got {other:?} ({other})").into());
        }
    }
    Ok(())
}

/// A failure that named no HTTP status stays a transport failure — the
/// classification is not a rename of every non-zero exit.
#[test]
fn a_failure_naming_no_status_stays_a_transport_failure() -> TestResult {
    let script = ScriptedGh::install("no-status", HEAD)?;
    script.configure(Scenario::new(HEAD).fail_reads(1))?;
    let gh = script.binding("acme/widgets", 64 * 1024)?;

    let error = lgwks_bot::rt::runtime::Runtime::new()?
        .block_on(gh.read_reviews(&pull()?))
        .err()
        .ok_or("a refused read must be an error")?;
    assert!(
        matches!(error, GhError::Transport { .. }),
        "an unclassifiable failure is a transport failure: {error:?}"
    );
    assert!(
        !matches!(error, GhError::Unauthorized { .. }),
        "and it is never guessed into a permission outcome"
    );
    Ok(())
}

/// A changed-file inventory past its ceiling is a typed coverage refusal.
#[test]
fn a_diff_past_its_file_ceiling_is_a_typed_coverage_refusal() -> TestResult {
    let script = ScriptedGh::install("files-ceiling", HEAD)?;
    let over = u32::try_from(lgwks_bot::domain::gh::MAX_DIFF_FILES_PER_PULL)?.saturating_add(1);
    script.configure(Scenario::new(HEAD).with_filler_files(over))?;
    let gh = script.binding("acme/widgets", 4 * 1024 * 1024)?;

    let error = lgwks_bot::rt::runtime::Runtime::new()?
        .block_on(gh.read_diff(&pull()?))
        .err()
        .ok_or("a diff past the file ceiling must be refused")?;
    assert!(
        error.is_coverage_incomplete(),
        "either diff ceiling is a coverage refusal: {error}"
    );
    match error {
        GhError::DiffFileCeiling { files, ceiling, .. } => {
            assert_eq!(files, usize::try_from(over)?);
            assert_eq!(ceiling, lgwks_bot::domain::gh::MAX_DIFF_FILES_PER_PULL);
        }
        other => {
            return Err(format!("expected DiffFileCeiling, got {other:?} ({other})").into());
        }
    }
    Ok(())
}

/// A server that declines to render the diff is a typed coverage refusal.
#[test]
fn an_unavailable_diff_is_a_typed_coverage_refusal() -> TestResult {
    let script = ScriptedGh::install("diff-gone", HEAD)?;
    script.configure(Scenario::new(HEAD).diff_unavailable())?;
    let gh = script.binding("acme/widgets", 64 * 1024)?;

    let error = lgwks_bot::rt::runtime::Runtime::new()?
        .block_on(gh.read_diff(&pull()?))
        .err()
        .ok_or("an unavailable diff must be refused")?;
    assert!(
        error.is_coverage_incomplete(),
        "an unavailable diff is a coverage refusal: {error}"
    );
    assert!(
        matches!(error, GhError::DiffUnavailable { .. }),
        "and it is its own typed arm, not a transport failure: {error:?}"
    );
    Ok(())
}

/// The changed-file inventory is read as data and carries each file's metadata.
///
/// The build script's patch text would remove the marker file if anything ran
/// it; nothing does, and the assertion is that the patch is *text* the adapter
/// read rather than a program it executed.
#[test]
fn a_changed_file_inventory_is_read_as_data() -> TestResult {
    let script = ScriptedGh::install("inventory", HEAD)?;
    script.configure(Scenario::new(HEAD).hostile_build_script())?;
    let gh = script.binding("acme/widgets", 64 * 1024)?;

    let diff = lgwks_bot::rt::runtime::Runtime::new()?.block_on(gh.read_diff(&pull()?))?;
    assert_eq!(
        diff.number(),
        7,
        "the inventory names the pull request it was read for"
    );
    assert_eq!(diff.files().len(), 1, "one changed file");
    assert!(!diff.is_empty(), "an inventory with a file is not empty");
    let file = diff.files().first().ok_or("the one changed file")?;
    assert_eq!(file.filename(), "build.rs");
    assert_eq!(file.status(), "added");
    assert_eq!(file.additions(), 1);
    assert_eq!(file.deletions(), 0);
    assert!(
        file.patch()
            .is_some_and(|patch| patch.contains("LGWKS_TEST_MARKER")),
        "the patch text is read as data: {:?}",
        file.patch()
    );
    assert!(
        diff.patch_bytes() > 0,
        "the inventory reports the patch bytes it carries: {}",
        diff.patch_bytes()
    );

    // The empty default inventory is empty and carries no patch bytes.
    let bare = ScriptedGh::install("inventory-empty", HEAD)?;
    bare.configure(Scenario::new(HEAD))?;
    let gh = bare.binding("acme/widgets", 64 * 1024)?;
    let empty = lgwks_bot::rt::runtime::Runtime::new()?.block_on(gh.read_diff(&pull()?))?;
    assert!(
        empty.is_empty(),
        "a pull request with no changed files is an empty inventory"
    );
    assert_eq!(empty.patch_bytes(), 0);
    Ok(())
}

// ── A refused credential is a typed repair on the verb path ─────────────────

/// Row 3 of #278 on the shipped adapter: a `gh` whose token the receiver
/// refuses, polled through the `Observe` verb a bot actually runs, reports
/// `CredentialRejected` carrying the re-grant — not the generic domain failure
/// a retry policy would repeat.
#[test]
fn a_refused_credential_reaches_the_observe_verb_as_its_repair() -> TestResult {
    use lgwks_bot::cap::Cap;
    use lgwks_bot::domain::gh::PrSnapshotSource;
    use lgwks_bot::ecs::EcsError;
    use lgwks_bot::error::RetryClass;
    use lgwks_bot::gate::GrantSet;
    use lgwks_bot::spec::NeedSet;
    use lgwks_bot::verb::Observe;

    let script = ScriptedGh::install("credential", HEAD)?;
    script.configure(Scenario::new(HEAD).deny_credential(401))?;
    let gh = gh_for(&script, 64 * 1024, Duration::from_secs(10))?;
    let source = PrSnapshotSource::new(gh, PullRequest::new(Repository::new("acme/widgets")?, 7));
    let auth = GrantSet::empty()
        .grant(Cap::net())
        .grant(Cap::sys())
        .issue(source.required_caps())?;

    let polled = lgwks_bot::rt::runtime::Runtime::new()?.block_on(source.poll((auth, ())));
    let Err(rejection) = polled else {
        return Err(format!("a refused credential must not read a snapshot: {polled:?}").into());
    };
    assert_eq!(
        rejection.retry_class(),
        RetryClass::Never,
        "the same token against the same receiver is refused again: {rejection}"
    );
    let EcsError::CredentialRejected {
        ref domain,
        status,
        ref needs,
    } = rejection
    else {
        return Err(format!("a 401 must be CredentialRejected, not {rejection}").into());
    };
    assert_eq!(
        domain, "gh::pr_snapshot",
        "attributed to the verb that saw it"
    );
    assert_eq!(status, 401, "with the status the receiver named");
    assert_eq!(
        needs,
        &NeedSet::expired_credentials("gh::pr_snapshot", &[Cap::net()]),
        "and the repair names the network authority the token stands for"
    );
    Ok(())
}
