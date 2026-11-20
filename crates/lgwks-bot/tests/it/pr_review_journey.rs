//! Black-box acceptance for the PR-review journey through a real `gh` process
//! boundary.
//!
//! Every test here drives [`lgwks_bot::review::review_pr`] through a
//! [`Host`](lgwks_bot::task::Host), and every one of them forks real processes:
//! a scripted `gh` placed first on `PATH`, recording its argv and answering from
//! a behaviour file. Nothing is replaced inside the process; the code under test
//! admits, spawns, captures, applies its deadline and reaps exactly as it would
//! for the real client.
//!
//! What is pinned here:
//!
//! - the create carries `commit_id` equal to the pinned head, and the read-back
//!   is a **separate** invocation (PR-06, PR-09);
//! - a lost response after an accepted write is reconciled by reading back, and
//!   argv records exactly **one** create (PR-07);
//! - a head change is a typed refusal naming both shas, with no create at all;
//! - a non-zero exit is not a published review;
//! - two identities on one repository stay isolated;
//! - a truncated capture is reported as truncated, and a hanging client is
//!   stopped as a whole process group.
//!
//! The tests live in children by behaviour: `publish`, `lost_response`,
//! `subject` and `partial`; the shared builders are here.
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

use std::num::NonZeroUsize;
use std::time::Duration;

use lgwks_bot::domain::gh::{Gh, PullRequest, Repository, ReviewComment};
use lgwks_bot::review::{ReviewOutcome, ReviewRequest};
use lgwks_bot::script::{FlowError, Scope};
use lgwks_bot::task::{Host, Report, Task, task};

use crate::scripted_gh::ScriptedGh;

mod lost_response;
mod partial;
mod publish;
mod subject;

/// What a test reports when a precondition did not hold.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The head every scenario starts from.
const HEAD: &str = "1111111111111111111111111111111111111111";

/// The head a moved-head scenario reports on its second read.
const MOVED_HEAD: &str = "2222222222222222222222222222222222222222";

/// The pull request under review: `owner/repo#7`.
fn pull() -> Result<PullRequest, Box<dyn std::error::Error>> {
    Ok(PullRequest::new(Repository::new("acme/widgets")?, 7))
}

/// A host with enough admission for a couple of concurrent runs and a deadline
/// generous enough that a test fails on a real hang rather than on the clock.
fn host() -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::builder("reviewer")?
        .max_concurrent_tasks(NonZeroUsize::new(8).ok_or("a non-zero ceiling")?)
        .default_deadline(Duration::from_secs(30))
        .build()?)
}

/// The `Gh` binding pointed at the scripted client.
///
/// The client is named `gh` and located through the `PATH` this binding puts in
/// front of every child, so the test exercises real program resolution rather
/// than pinning an absolute path — which is what a deployment does. The `PATH`
/// is a per-child delta rather than a mutation of this process's environment,
/// which keeps the tests independent and needs no `unsafe`.
fn gh_for(scripted: &ScriptedGh) -> Result<Gh, Box<dyn std::error::Error>> {
    gh_with_capture(scripted, 64 * 1024)
}

/// The same binding with a chosen capture ceiling.
///
/// The diff-bound probes need an answer larger than the default ceiling, and a
/// per-test copy of the binding would be a place for one probe to quietly stop
/// running the journey the others run.
fn gh_with_capture(
    scripted: &ScriptedGh,
    capture: usize,
) -> Result<Gh, Box<dyn std::error::Error>> {
    Ok(scripted.binding("acme/widgets", capture)?)
}

/// A script that emits a fixed body, standing in for a real analysis stage.
///
/// It is a closure over its own captured state rather than a named function,
/// which is the shape a caller's analysis takes: no trait to implement, no
/// box, and the pinned snapshot it is handed is the only input.
fn body_script(
    body: &'static str,
) -> impl Fn(&lgwks_bot::domain::gh::PrSnapshot, &Scope) -> Result<String, FlowError> {
    move |_snapshot, _scope| Ok(String::from(body))
}

/// A script that always fails, to prove a failed analysis publishes nothing.
fn failing_script()
-> impl Fn(&lgwks_bot::domain::gh::PrSnapshot, &Scope) -> Result<String, FlowError> {
    |_snapshot, _scope| Err(FlowError::failed("the analysis found no eligible findings"))
}

/// The task, host and request every journey scenario in this file starts from.
///
/// One place, because the shape of a run *is* the thing under test: a task
/// body that pinned its own subject, published outside a host, or reconciled
/// by re-posting would be a different journey, not a variation of this one.
/// Each scenario supplies the analysis stage; everything else is shared.
fn review_run() -> Result<ReviewRun, Box<dyn std::error::Error>> {
    let host = host()?;
    let request = ReviewRequest::new(pull()?, "COMMENT", REVIEW_BODY).with_marker("lgwks-test-run");
    let body: ReviewTaskBody = review_body;
    let task = task("review-pr", body)?;
    Ok(ReviewRun {
        host,
        task,
        request,
    })
}

/// The same run, with an analysis stage that always fails.
fn failing_review_run() -> Result<ReviewRun, Box<dyn std::error::Error>> {
    let host = host()?;
    let request = ReviewRequest::new(pull()?, "COMMENT", REVIEW_BODY).with_marker("lgwks-test-run");
    let body: ReviewTaskBody = failing_body;
    let task = task("review-pr", body)?;
    Ok(ReviewRun {
        host,
        task,
        request,
    })
}

/// The shared run with `count` inline comments declared on the request.
///
/// The partial-submission probe needs the request to intend more comments than
/// the receiver lands; the journey carries the count to the payload so the
/// reconciliation can compare applied against intended.
fn review_run_with_comments(count: u64) -> Result<ReviewRun, Box<dyn std::error::Error>> {
    let comments = (0..count)
        .map(|index| {
            ReviewComment::new(
                format!("src/file{index}.rs"),
                index.saturating_add(1),
                format!("comment {index}"),
            )
        })
        .collect();
    let host = host()?;
    let request = ReviewRequest::new(pull()?, "COMMENT", REVIEW_BODY)
        .with_marker("lgwks-test-run")
        .with_comments(comments);
    let body: ReviewTaskBody = review_body;
    let task = task("review-pr", body)?;
    Ok(ReviewRun {
        host,
        task,
        request,
    })
}

/// Run the shared task against `scripted` and return its report.
///
/// One call, because the two lines it replaces are not incidental: binding the
/// `Gh` to the scripted client and executing under the host is what "the
/// journey runs through the adapter and the front door" means. A per-test copy
/// of those two lines is a place for one test to quietly stop doing it.
fn run_passing(scripted: &ScriptedGh) -> Result<Report<ReviewOutcome>, Box<dyn std::error::Error>> {
    let run = review_run()?;
    let gh = gh_for(scripted)?;
    Ok(run.host.block_on(&run.task, (gh, run.request))?)
}

/// The shared run against `scripted`, asserted to have succeeded; `why` is what
/// a failure of that assertion says the success stood for.
fn run_succeeding(
    scripted: &ScriptedGh,
    why: &str,
) -> Result<Report<ReviewOutcome>, Box<dyn std::error::Error>> {
    let report = run_passing(scripted)?;
    assert_eq!(
        report.disposition().label(),
        "Succeeded",
        "{why}: {}",
        error_text(&report)
    );
    Ok(report)
}

/// The run's error rendered for an assertion message, empty when it has none.
fn error_text(report: &Report<ReviewOutcome>) -> String {
    match report.error() {
        Some(error) => error.to_string(),
        None => String::new(),
    }
}

/// The task body a passing scenario runs: publish [`REVIEW_BODY`] at the pinned
/// head and verify it.
fn review_body(scope: Scope, (gh, request): (Gh, ReviewRequest)) -> ReviewFuture {
    Box::pin(async move {
        lgwks_bot::review::review_pr(scope, gh, request, body_script(REVIEW_BODY)).await
    })
}

/// The task body a scenario that needs a failing analysis runs instead.
fn failing_body(scope: Scope, (gh, request): (Gh, ReviewRequest)) -> ReviewFuture {
    Box::pin(
        async move { lgwks_bot::review::review_pr(scope, gh, request, failing_script()).await },
    )
}

/// The three values every scenario starts from.
///
/// Returned together so a test reads as the difference between itself and the
/// shared world rather than as three lines of setup copied into each one — the
/// shape of a run *is* the thing under test, and a per-test copy of it is a
/// place for one test to quietly stop running it the way the others do.
struct ReviewRun {
    /// The host the run executes under.
    host: Host,
    /// The task to run.
    task: Task<ReviewTaskBody>,
    /// The request it runs over.
    request: ReviewRequest,
}

/// The closure shape both [`review_run`] and [`failing_review_run`] build.
///
/// Named so a scenario can hold the run in one type whatever analysis stage it
/// chose; the two stages differ only in the body they pass to `review_pr`.
type ReviewTaskBody = fn(Scope, (Gh, ReviewRequest)) -> ReviewFuture;

/// The future a task body in this file returns.
type ReviewFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ReviewOutcome, lgwks_bot::script::FlowError>>>,
>;

/// The body the shared scenarios publish, so a read-back and a payload compare
/// like with like.
const REVIEW_BODY: &str = "two findings";

/// The staged payload of the nth create, or `None` when there was none.
fn payload_of(scripted: &ScriptedGh, index: usize) -> std::io::Result<Option<String>> {
    let calls = scripted.calls()?;
    let creates = calls
        .iter()
        .filter(|argv| {
            argv.windows(2)
                .any(|pair| matches!(pair, [flag, verb] if flag == "--method" && verb == "POST"))
        })
        .count();
    if index >= creates {
        return Ok(None);
    }
    let received = scripted.received()?;
    Ok(received.get(index).cloned())
}
