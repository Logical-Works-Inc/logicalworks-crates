//! Deliberately wrong solution — `aggregate`, unbounded fan-out.
//!
//! This is a mutant input for the harness, not an example: it ignores the
//! four-in-flight bound and starts every fetch at once. The oracle's
//! `at_most_four_fetches_are_in_flight` test must fail it. It is never built by
//! the estate workspace (this directory is not a member), so it is not held to
//! the estate lint contract.

use std::time::Duration;

use ai_task_support::{FetchError, Fetcher};
use lgwks_bot::rt::task::{FanOutError, join_all_bounded};

#[derive(Debug)]
pub enum SolveError {
    Fetch { id: u32 },
    Deadline,
    FanOut(FanOutError),
}

/// The deadline is accepted for the shared signature and ignored on purpose:
/// ignoring it is part of what makes this the wrong solution.
pub async fn solve(ids: Vec<u32>, fetch: Fetcher, _deadline: Duration) -> Result<u64, SolveError> {
    let futures = ids.into_iter().map(|id| {
        let fetcher = fetch.clone();
        async move { fetcher.fetch(id).await }
    });
    let results = join_all_bounded(usize::MAX, futures)
        .await
        .map_err(SolveError::FanOut)?;
    let mut sum: u64 = 0;
    for result in results {
        sum = sum.saturating_add(result.map_err(|error| SolveError::Fetch { id: error.id() })?);
    }
    Ok(sum)
}
