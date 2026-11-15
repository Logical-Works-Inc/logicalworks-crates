//! The review list: its ceiling, its pages and its malformed answers.

use std::time::Duration;

use lgwks_bot::domain::gh::{GhError, PullRequest, Repository};

use crate::scripted_gh::{HANG_BOUND, Scenario, ScriptedGh};

use super::{HEAD, TestResult, gh_for};

// ── The review ceiling ──────────────────────────────────────────────────────

#[test]
fn a_review_list_past_the_ceiling_is_refused_not_truncated() -> TestResult {
    let script = ScriptedGh::install("ceiling", HEAD)?;
    // One more review than the adapter will decode. A client that paginates
    // happily produces exactly this, and the question is not whether the bytes
    // fit — they do — but whether a *clean decode* of a partial history is
    // reported as the history.
    let over = u32::try_from(lgwks_bot::domain::gh::MAX_REVIEWS_PER_PULL)
        .map_err(|_| "the ceiling must fit the fixture's counter")?
        .saturating_add(1);
    script.configure(Scenario::new(HEAD).with_filler_reviews(over))?;
    let gh = gh_for(&script, 4 * 1024 * 1024, HANG_BOUND)?;

    let refused = lgwks_bot::Runtime::new()?
        .block_on(async {
            gh.read_reviews(&PullRequest::new(Repository::new("acme/widgets")?, 7))
                .await
        })
        .err()
        .ok_or("a list past the ceiling must be refused, not returned short")?;

    match refused {
        GhError::ReviewCeiling {
            ref path,
            reviews,
            ceiling,
        } => {
            assert!(
                path.contains("/reviews"),
                "the refusal names the endpoint it came from: {path}"
            );
            assert_eq!(
                reviews,
                usize::try_from(over)?,
                "the refusal reports what the client actually returned, so a caller \
                 can tell a long history from a short one"
            );
            assert_eq!(
                ceiling,
                lgwks_bot::domain::gh::MAX_REVIEWS_PER_PULL,
                "and the ceiling that refused it, which is a declared bound"
            );
        }
        other => {
            return Err(format!(
                "a list past the ceiling must be ReviewCeiling, not {other:?} ({other})"
            )
            .into());
        }
    }
    assert!(
        refused.to_string().contains("not returned"),
        "the message says the list was withheld rather than shortened: {refused}"
    );
    Ok(())
}

#[test]
fn a_review_list_exactly_at_the_ceiling_is_read() -> TestResult {
    let script = ScriptedGh::install("ceiling-edge", HEAD)?;
    // The boundary: exactly the ceiling is the largest list the adapter accepts,
    // and refusing it would make the bound a fiction rather than a declaration.
    let at = u32::try_from(lgwks_bot::domain::gh::MAX_REVIEWS_PER_PULL)
        .map_err(|_| "the ceiling must fit the fixture's counter")?;
    script.configure(Scenario::new(HEAD).with_filler_reviews(at))?;
    let gh = gh_for(&script, 4 * 1024 * 1024, HANG_BOUND)?;

    let reviews = lgwks_bot::Runtime::new()?.block_on(async {
        gh.read_reviews(&PullRequest::new(Repository::new("acme/widgets")?, 7))
            .await
    })?;
    assert_eq!(
        reviews.len(),
        lgwks_bot::domain::gh::MAX_REVIEWS_PER_PULL,
        "the ceiling is inclusive: exactly the declared number is accepted"
    );
    Ok(())
}

/// T31, pagination: a review list served in pages is read across every page,
/// and an answer whose pages arrive as separate documents is refused rather
/// than decoded as its first page.
///
/// The script serves 100 records a page and answers a read that does not ask
/// for `--paginate` with the first page only, exactly as the REST endpoint
/// does for a client that ignores the `Link` header. So the record that sits
/// past the second page boundary is visible only to an adapter that follows
/// every page: one that read a single page would decode a clean list of 100
/// and call it complete. The second half is the older client's spelling, one
/// array per page back to back: a decoder that stopped at the first closing
/// bracket would return page one as the whole history, so the answer must be
/// a typed refusal.
#[test]
fn a_paginated_review_list_is_read_across_every_page_t31() -> TestResult {
    const PAGE: u32 = 100;
    const RECORDS: u32 = 250;
    let script = ScriptedGh::install("paginated", HEAD)?;
    script.configure(
        Scenario::new(HEAD)
            .with_filler_reviews(RECORDS)
            .with_review_pages(PAGE),
    )?;
    let gh = gh_for(&script, 4 * 1024 * 1024, HANG_BOUND)?;
    let pull = PullRequest::new(Repository::new("acme/widgets")?, 7);
    let runtime = lgwks_bot::Runtime::new()?;

    let reviews = runtime.block_on(gh.read_reviews(&pull))?;
    assert_eq!(
        reviews.len(),
        usize::try_from(RECORDS)?,
        "every page was read: three pages of {PAGE}, the last one short"
    );
    let last = reviews
        .last()
        .ok_or("a read of 250 records returned none")?;
    assert_eq!(
        last.id(),
        7_000_u64.saturating_add(u64::from(RECORDS).saturating_sub(1)),
        "the last record is the one on the third page, which a one-page read never sees"
    );

    script.configure(
        Scenario::new(HEAD)
            .with_filler_reviews(RECORDS)
            .with_review_pages(PAGE)
            .answers_reviews_with("concatenated"),
    )?;
    match runtime.block_on(gh.read_reviews(&pull)) {
        Err(GhError::Response { .. } | GhError::MalformedResponse { .. }) => {}
        other => {
            let refusal = Err(format!(
                "pages arriving as separate documents must be refused, never decoded as page \
                 one: got {other:?}"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "a_paginated_review_list_is_read_across_every_page_t31: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// The scripted receiver's read-back never hands out a record another run is
/// still appending. On tmpfs a concurrent reader can see the front half of an
/// append (CI read a store cut mid-string at byte 8,193), so the fixture
/// commits each record with its trailing newline and reads only committed
/// lines. Without that, the adapter would see a torn document, and a
/// saturation run whose own review landed fine would be reported `Unknown`.
#[test]
fn a_record_still_being_appended_is_not_read_back() -> TestResult {
    let script = ScriptedGh::install("torn-append", HEAD)?;
    script.configure(Scenario::new(HEAD))?;
    std::fs::write(
        script.dir().join("reviews.jsonl"),
        format!(
            ",{{\"id\":9001,\"commit_id\":\"{HEAD}\",\"state\":\"COMMENTED\",\"body\":\"b\",\"comment_count\":0}}\n\
             ,{{\"id\":9002,\"commit_id\":\"{HEAD}\",\"sta"
        ),
    )?;
    let gh = gh_for(&script, 64 * 1024, Duration::from_secs(10))?;

    let reviews = lgwks_bot::Runtime::new()?.block_on(async {
        gh.read_reviews(&PullRequest::new(Repository::new("acme/widgets")?, 7))
            .await
    })?;
    assert_eq!(
        reviews.len(),
        1,
        "the committed record is read and the half-appended one is not: {reviews:?}"
    );
    Ok(())
}

#[test]
fn a_malformed_review_list_is_refused_rather_than_decoded_into_a_partial_answer() -> TestResult {
    for shape in ["garbage", "truncated"] {
        let script = ScriptedGh::install(shape, HEAD)?;
        script.configure(Scenario::new(HEAD).answers_reviews_with(shape))?;
        let gh = gh_for(&script, 64 * 1024, Duration::from_secs(10))?;

        let refused = lgwks_bot::Runtime::new()?
            .block_on(async {
                gh.read_reviews(&PullRequest::new(Repository::new("acme/widgets")?, 7))
                    .await
            })
            .err()
            .ok_or("a review list that is not a review list must be refused")?;
        assert!(
            matches!(refused, GhError::MalformedResponse { .. }),
            "{shape:?} must be a decode failure, not a partial list: {refused:?}"
        );
        assert!(
            refused.is_read_only_retryable(),
            "and it is read-only retryable, because a malformed answer says nothing \
             about whether the reviews exist: {refused}"
        );
    }
    Ok(())
}
