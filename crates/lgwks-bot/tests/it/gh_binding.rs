//! The adapter's own guarantees, driven through the same real process
//! boundary: bounded capture, a deadline that stops a whole process group, and
//! the refusals a caller must be able to distinguish.
//!
//! These are properties of the *binding*, not of the review journey, so they
//! are exercised directly against [`Gh`] rather than through a task. Every one
//! of them forks real processes: the fixture is the same receiver-backed
//! scripted `gh`, and the observations are what the adapter reports about what
//! the child actually did.
//!
//! The tests live in children by behaviour: `capture` (bounded capture and the
//! deadline), `validation` (what is refused before anything runs), `reviews`
//! (the review list's ceiling, pages and malformed answers) and `refusals`
//! (typed refusals the review journey depends on).
// `ephemeral` is required, not incidental: a staged publication payload needs a
// filename no two concurrent publications share, and without that feature the
// binding refuses to publish rather than fall back to a name the OS reuses. The
// `full` lane runs this suite, so the refusal is exercised rather than
// assumed; `gh_binding` covers what a process-only build can still do.
#![cfg(all(
    unix,
    feature = "script",
    feature = "process",
    feature = "time",
    feature = "sync",
    feature = "ephemeral"
))]

use std::time::Duration;

use lgwks_bot::domain::gh::{Gh, GhOutcome};
use lgwks_bot::ecs::EcsError;

use crate::scripted_gh::{BindingError, ScriptedGh};

mod capture;
mod refusals;
mod reviews;
mod validation;

/// What a test reports when a precondition did not hold.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The head every scenario reads.
const HEAD: &str = "1111111111111111111111111111111111111111";

/// The repository every scenario binds.
const REPOSITORY: &str = "acme/widgets";

/// A binding to `script` for [`REPOSITORY`], with a non-zero capture ceiling
/// (an unbounded capture is not expressible) and the named deadline.
fn gh_for(script: &ScriptedGh, capture: usize, deadline: Duration) -> Result<Gh, BindingError> {
    Ok(script
        .binding(REPOSITORY, capture)?
        .deadline(Some(deadline)))
}

/// The one extension the fixture needs so a test can read the raw outcome
/// without going through a typed verb call that would reject a truncated or a
/// hung answer first.
trait LossyQuery {
    /// Run one `gh api` call through the public verb surface and report what
    /// was observed.
    ///
    /// The `Result` is the adapter's own refusal, not a test convenience: a
    /// probe that needs an outcome asserts the call produced one, and a probe
    /// that expects a refusal asserts the error — which is how a test tells
    /// "the call was refused" apart from "the call ran and was truncated".
    type Refused;

    /// The observed outcome of one call.
    fn query_lossy(&self, path: &str) -> Self::Refused;
}

impl LossyQuery for Gh {
    /// The future [`LossyQuery::query_lossy`] returns.
    type Refused =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<GhOutcome, EcsError>>>>;

    fn query_lossy(&self, path: &str) -> Self::Refused {
        let args = self.args_for(&["--method", "GET", path]);
        // `GhQuery` is the public verb surface, so this reaches the same
        // supervised call a real consumer makes rather than a private one.
        let query = lgwks_bot::domain::gh::GhQuery::new(self.clone(), args.into_iter().skip(1));
        // The adapter's own requirement list is what the proof is built from,
        // so the test cannot drift from what the binding actually demands: a
        // new capability on the binding would deny this call rather than
        // silently passing it.
        let required = self.required_caps().to_vec();
        let grants = required
            .iter()
            .fold(lgwks_bot::gate::GrantSet::empty(), |set, cap| {
                set.grant(cap.clone())
            });
        Box::pin(async move {
            let auth = grants.issue(&required)?;
            lgwks_bot::verb::Query::query(&query, (auth, &())).await
        })
    }
}
