//! Outside calls through the public run path (#783).
//!
//! | Requirement | Test |
//! |---| Facet |
//! | `fetch` deadline | `fetch_deadline_refuses_a_slow_upstream` |
//! | `fetch` dropped connection | `fetch_reports_a_connection_dropped_mid_body` |
//! | `fetch` 5xx storm | `fetch_stops_a_5xx_storm_at_the_budget` |
//! | `fetch` oversize | `fetch_refuses_an_oversize_body_while_streaming` |
//! | `fetch` POST policy | `fetch_post_runs_under_an_allowing_policy_and_parks_under_the_default` |
//! | `ask` truncation | `ask_refuses_truncated_model_output` |
//! | `ask` undecodable | `ask_refuses_undecodable_output_for_the_declared_type` |
//! | `ask` decode | `ask_decodes_the_declared_type` |
//! | `send` intent crash | `send_dispatches_exactly_once_across_an_intent_dispatch_crash` |
//! | `send` settlement crash | `send_after_a_dispatch_crash_settles_from_evidence_and_never_redispatches` |
//! | `send` unknown | `send_stays_unknown_until_late_evidence_settles_it` |
//! | `send` refused | `send_retries_after_a_refused_dispatch` |
//! | `send` policy | `send_without_a_matching_approval_is_refused` |
//! | `send` durability | `send_without_a_store_is_refused` |
//! | capabilities | `capability_shortfall_is_refused_before_the_first_step_naming_every_grant` |
//! | tenancy | `two_tenants_keep_separate_budgets` |
//! | plan | `the_plan_exposes_every_outbound_reach` |
//!
//! Every flow below is expanded by the real macro and run by a real `Host`
//! over a real run-store file and a real loopback upstream; the scratch
//! directory is `crate::scratch`, so no run inherits another's records. Each
//! test names the requirement it proves in its table row above.
//!
//! The error type is an enum over the run path's own refusals, so a failure
//! names the fact that moved rather than a string.

#![cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::proposal::{Shape, StubModel};
use lgwks_bot::script::{AskRequest, FetchOptions, FlowError, ModelOutput, ModelPort, PostBody};
use lgwks_bot::script::{Scope, SendOutcome};
use lgwks_bot::task::{Disposition, Host, task};
use lgwks_bot::{Cap, GrantSet};

use crate::effect_policy::allowing;
use crate::outside_support::{ARCHITECTURE, calls, get, mail, mail_crash};
use crate::outside_support::{Behavior, OutsideError, StubMailer, Upstream};
use crate::outside_support::{fail, host_on, is_applied, is_unknown, outside_grants, run_of};
use crate::scratch::Scratch;

/// A test's result: an error fails it with the refusal behind it.
type TestResult = Result<(), OutsideError>;

/// The `mail_crash` input: the mailer, the payload, and whether to crash
/// after the dispatch.
type CrashInput = (Arc<StubMailer>, Vec<u8>, bool);

/// One run of `mail_crash`, boxed so a task over it has a type a helper names.
type CrashRun = std::pin::Pin<Box<dyn Future<Output = Result<SendOutcome, FlowError>>>>;

/// The `mail_crash` flow as a task a host runs and resumes.
type CrashTask = lgwks_bot::task::Task<fn(Scope, CrashInput) -> CrashRun>;

/// One run of `mail_crash`.
fn crash_run(scope: Scope, input: CrashInput) -> CrashRun {
    Box::pin(async move {
        let (mailer, payload, crash) = input;
        mail_crash(&scope, &mailer, payload, crash).await
    })
}

/// Run `mail_crash` once over `dir`, crashing after its dispatch: the run it
/// left and the task that resumes it.
fn crash_once(
    dir: &std::path::Path,
    mailer: &Arc<StubMailer>,
) -> Result<(lgwks_bot::effect::RunId, CrashTask), OutsideError> {
    let body: fn(Scope, CrashInput) -> CrashRun = crash_run;
    let crashing = task("crash", body)?;
    let first = host_on(dir, "acme", outside_grants())?;
    let crashed = lgwks_bot::block_on(allowing(
        first.run(&crashing, (Arc::clone(mailer), b"hello".to_vec(), true)),
    ));
    if crashed.error().is_none() {
        return fail("the crashing run fails after its dispatch");
    }
    Ok((run_of(&crashed)?, crashing))
}

/// Resume `run` without the crash on a fresh host over `dir`: the restart a
/// crash leaves behind.
fn resume_crashed(
    dir: &std::path::Path,
    run: lgwks_bot::effect::RunId,
    crashing: &CrashTask,
    mailer: &Arc<StubMailer>,
) -> Result<lgwks_bot::task::Report<SendOutcome>, OutsideError> {
    let host = host_on(dir, "acme", outside_grants())?;
    Ok(lgwks_bot::block_on(allowing(host.resume(
        run,
        crashing,
        (Arc::clone(mailer), b"hello".to_vec(), false),
    ))))
}

/// The first seed whose [`StubModel`] draws `shape`, if one draws it in the
/// first thousand seeds.
fn seed_with(shape: Shape) -> Result<u64, OutsideError> {
    for seed in 0..1_000 {
        if StubModel::from_seed(seed).shape() == shape {
            return Ok(seed);
        }
    }
    fail("no seed drew the shape")
}

/// The status a [`FlowError::Exhausted`] names in its last failure, when it is
/// one of the fetch failures.
fn exhausted_last_status(error: &FlowError) -> Option<u16> {
    match *error {
        FlowError::Exhausted { ref last, .. } => match **last {
            FlowError::Transient { ref reason, .. } => status_in(reason),
            _ => None,
        },
        _ => None,
    }
}

/// The three-digit status a fetch failure reason names, if one is there.
fn status_in(reason: &str) -> Option<u16> {
    reason
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| part.len() == 3)
        .filter_map(|part| part.parse().ok())
        .next()
}

/// A slow upstream: the deadline gives a typed timeout, and the exchange
/// ends instead of hanging on the socket.
#[test]
fn fetch_deadline_refuses_a_slow_upstream() -> TestResult {
    let scratch = Scratch::new("outside-slow")?;
    let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
    let upstream = Upstream::serve(Behavior::Slow, 1, Duration::from_secs(1))?;
    let fetching = task("slow", |scope: Scope, url: String| async move {
        let options = FetchOptions::default().deadline(Duration::from_millis(300));
        get(&scope, &url, &options).await
    })?;
    let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
    upstream.join();
    if !matches!(report.error(), Some(FlowError::TimedOut { .. })) {
        return fail("a slow upstream is a typed timeout");
    }
    Ok(())
}

/// A connection dropped mid-body: the exchange spends its attempts and ends
/// exhausted, dialling once per attempt and never more.
#[test]
fn fetch_reports_a_connection_dropped_mid_body() -> TestResult {
    let scratch = Scratch::new("outside-drop")?;
    let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
    let upstream = Upstream::serve(Behavior::DropMidBody, 2, Duration::ZERO)?;
    let fetching = task("drop", |scope: Scope, url: String| async move {
        let options = FetchOptions::default().attempts(2);
        get(&scope, &url, &options).await
    })?;
    let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
    let hits = upstream.hits();
    upstream.join();
    if !matches!(
        report.error(),
        Some(FlowError::Exhausted { attempts: 2, .. })
    ) {
        return fail("a dropped connection spends its attempts, then exhausts");
    }
    if hits != 2 {
        return fail("two attempts dial twice, never more");
    }
    Ok(())
}

/// A 5xx storm: retries stop at the word's budget, the error names the last
/// status, and the upstream saw exactly the budgeted attempts.
#[test]
fn fetch_stops_a_5xx_storm_at_the_budget() -> TestResult {
    let scratch = Scratch::new("outside-storm")?;
    let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
    let upstream = Upstream::serve(Behavior::Storm, 3, Duration::ZERO)?;
    let fetching = task("storm", |scope: Scope, url: String| async move {
        let options = FetchOptions::default().attempts(3);
        get(&scope, &url, &options).await
    })?;
    let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
    let hits = upstream.hits();
    upstream.join();
    match report.error() {
        Some(&FlowError::Exhausted { attempts: 3, .. }) => {}
        _ => return fail("a 5xx storm exhausts its three attempts"),
    }
    let status = match report.error() {
        Some(error) => exhausted_last_status(error),
        None => None,
    };
    if status != Some(500) {
        return fail("the exhausted error names the last 500");
    }
    if hits != 3 {
        return fail("three attempts dial three times, never more");
    }
    Ok(())
}

/// An oversize body: refused while it streams at the declared bound, never
/// buffered whole.
#[test]
fn fetch_refuses_an_oversize_body_while_streaming() -> TestResult {
    let scratch = Scratch::new("outside-huge")?;
    let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
    let upstream = Upstream::serve(Behavior::Oversized(256_000), 1, Duration::ZERO)?;
    let fetching = task("huge", |scope: Scope, url: String| async move {
        let options = FetchOptions::default().max_body_bytes(65_536);
        get(&scope, &url, &options).await
    })?;
    let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
    upstream.join();
    match report.error() {
        Some(&FlowError::InvalidBound { max: 65_536, .. }) => {}
        _ => return fail("an oversize body is refused at its bound"),
    }
    Ok(())
}

/// A POST under an allowing policy runs; under the default policy the same
/// POST is refused before it dials, naming the approval it owes.
#[test]
fn fetch_post_runs_under_an_allowing_policy_and_parks_under_the_default() -> TestResult {
    let scratch = Scratch::new("outside-post")?;
    let upstream = Upstream::serve(Behavior::Ok, 1, Duration::ZERO)?;
    let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
    let posting = task("post", |scope: Scope, url: String| async move {
        let options = FetchOptions::default().post(PostBody::new("text/plain", b"hook".to_vec()));
        let answered = lgwks_bot::script::fetch(&scope, "post", &url, &options).await?;
        Ok::<u16, FlowError>(answered.status())
    })?;
    let allowed = lgwks_bot::block_on(allowing(host.run(&posting, upstream.url())));
    upstream.join();
    if allowed.into_result()? != 200 {
        return fail("an allowed POST answers its status");
    }
    let scratch_denied = Scratch::new("outside-post-denied")?;
    let gated = host_on(
        scratch_denied.path(),
        "acme",
        GrantSet::empty().grant(Cap::net()),
    )?;
    let upstream_denied = Upstream::serve(Behavior::Ok, 0, Duration::ZERO)?;
    let denied = lgwks_bot::block_on(gated.run(&posting, upstream_denied.url()));
    let denied_hits = upstream_denied.hits();
    upstream_denied.join();
    if !matches!(denied.error(), Some(FlowError::NeedsApproval { .. })) {
        return fail("a gated POST without an approval is refused");
    }
    if denied_hits != 0 {
        return fail("a refused POST dials nothing");
    }
    Ok(())
}

/// A truncated model output — by seed shape and by token budget — is refused
/// typed, never decoded into a claim about the whole output.
#[test]
fn ask_refuses_truncated_model_output() -> TestResult {
    let scope = Scope::root(lgwks_bot::script::Tenant::new("acme")?);
    let seed = seed_with(Shape::Truncated)?;
    let model = StubModel::from_seed(seed);
    let request = AskRequest::default().max_tokens(u32::MAX);
    let refused: Result<String, FlowError> =
        lgwks_bot::block_on(lgwks_bot::script::ask(&scope, "cut", &model, &request));
    if !matches!(refused, Err(FlowError::InvalidBound { .. })) {
        return fail("a truncated shape is refused before decoding");
    }
    let seed = seed_with(Shape::WellFormed)?;
    let model = StubModel::from_seed(seed);
    let full = model.output().len();
    if full <= 16 {
        return fail("the well-formed shape must exceed the small budget");
    }
    let request = AskRequest::default().max_tokens(16);
    let refused: Result<Vec<u8>, FlowError> =
        lgwks_bot::block_on(lgwks_bot::script::ask(&scope, "budget", &model, &request));
    if !matches!(refused, Err(FlowError::InvalidBound { .. })) {
        return fail("output cut at the token budget is refused");
    }
    Ok(())
}

/// Output no declared type decodes is refused typed: JSON over prose.
#[test]
fn ask_refuses_undecodable_output_for_the_declared_type() -> TestResult {
    let scope = Scope::root(lgwks_bot::script::Tenant::new("acme")?);
    let seed = seed_with(Shape::WellFormed)?;
    let model = StubModel::from_seed(seed);
    let refused: Result<lgwks_bot::script::Json<Report>, FlowError> = lgwks_bot::block_on(
        lgwks_bot::script::ask(&scope, "json", &model, &AskRequest::default()),
    );
    if !matches!(refused, Err(FlowError::Failed { .. })) {
        return fail("prose is not JSON and must fail typed");
    }
    Ok(())
}

/// A model call decodes into the declared type: bytes whole, text, and JSON
/// through a stub port that answers JSON.
#[test]
fn ask_decodes_the_declared_type() -> TestResult {
    let scope = Scope::root(lgwks_bot::script::Tenant::new("acme")?);
    let seed = seed_with(Shape::WellFormed)?;
    let model = StubModel::from_seed(seed);
    let bytes: Vec<u8> = lgwks_bot::block_on(lgwks_bot::script::ask(
        &scope,
        "bytes",
        &model,
        &AskRequest::default(),
    ))?;
    if bytes != model.output() {
        return fail("bytes decode whole");
    }
    let text: String = lgwks_bot::block_on(lgwks_bot::script::ask(
        &scope,
        "text",
        &model,
        &AskRequest::default(),
    ))?;
    if text.as_bytes() != model.output() {
        return fail("text decodes the same bytes");
    }
    let stub = JsonModel;
    let report: lgwks_bot::script::Json<Report> = lgwks_bot::block_on(lgwks_bot::script::ask(
        &scope,
        "report",
        &stub,
        &AskRequest::default(),
    ))?;
    if report.value.code != 200 {
        return fail("JSON decodes into the declared report");
    }
    Ok(())
}

/// A crash between the intent claim and the dispatch: the intent is durable,
/// nothing dispatched, and the resume over the same store dispatches exactly
/// once.
#[test]
fn send_dispatches_exactly_once_across_an_intent_dispatch_crash() -> TestResult {
    let scratch = Scratch::new("outside-intent-crash")?;
    let mailer = Arc::new(StubMailer::new());
    let first = host_on(scratch.path(), "acme", outside_grants())?;
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    mailer.fail_next_read();
    let crashed = lgwks_bot::block_on(allowing(
        first.run(&sending, (Arc::clone(&mailer), b"hello".to_vec())),
    ));
    if crashed.error().is_none() {
        return fail("a failed read-back fails the step");
    }
    if mailer.calls() != 0 {
        return fail("a failed read-back dispatches nothing");
    }
    let run = run_of(&crashed)?;
    drop(first);
    let second = host_on(scratch.path(), "acme", outside_grants())?;
    let resumed = lgwks_bot::block_on(allowing(second.resume(
        run,
        &sending,
        (Arc::clone(&mailer), b"hello".to_vec()),
    )));
    settled_once(&resumed, &mailer)
}

/// The resumed send settled applied on the receiver's evidence, with exactly
/// one dispatch across the crash: never two and never zero.
fn settled_once(resumed: &lgwks_bot::task::Report<SendOutcome>, mailer: &StubMailer) -> TestResult {
    let Some(outcome) = resumed.output() else {
        return fail("the resume settles the send");
    };
    if !is_applied(outcome) {
        return fail("the resume settles on the evidence");
    }
    if mailer.calls() != 1 {
        return fail("one dispatch across the crash, never two and never zero");
    }
    Ok(())
}

/// A crash between dispatch and settlement: the resume over the same store
/// reads the evidence the receiver kept and settles, dispatching nothing
/// twice.
#[test]
fn send_after_a_dispatch_crash_settles_from_evidence_and_never_redispatches() -> TestResult {
    let scratch = Scratch::new("outside-dispatch-crash")?;
    let mailer = Arc::new(StubMailer::new());
    let (run, crashing) = crash_once(scratch.path(), &mailer)?;
    if mailer.calls() != 1 {
        return fail("the crashing run dispatched once");
    }
    let resumed = resume_crashed(scratch.path(), run, &crashing, &mailer)?;
    settled_once(&resumed, &mailer)
}

/// A lost dispatch reply ends the run unknown, and later evidence settles
/// it — with no second dispatch in between.
#[test]
fn send_stays_unknown_until_late_evidence_settles_it() -> TestResult {
    let scratch = Scratch::new("outside-unknown")?;
    let mailer = Arc::new(StubMailer::new());
    mailer.lose_next_reply();
    let (run, crashing) = crash_once(scratch.path(), &mailer)?;
    if mailer.calls() != 1 {
        return fail("the lost dispatch was attempted once");
    }
    let unknown = resume_crashed(scratch.path(), run, &crashing, &mailer)?;
    let Some(outcome) = unknown.output() else {
        return fail("the resume ends unknown");
    };
    if !is_unknown(outcome) {
        return fail("with no evidence anywhere the outcome stays unknown");
    }
    if mailer.calls() != 1 {
        return fail("an unknown outcome dispatches nothing twice");
    }
    mailer.forget();
    let still = resume_crashed(scratch.path(), run, &crashing, &mailer)?;
    let Some(outcome) = still.output() else {
        return fail("the second resume ends unknown");
    };
    if !is_unknown(outcome) {
        return fail("taking the evidence away keeps the outcome unknown");
    }
    if mailer.calls() != 1 {
        return fail("a second unknown resume dispatches nothing");
    }
    mailer.land_applied();
    let settled = resume_crashed(scratch.path(), run, &crashing, &mailer)?;
    let Some(outcome) = settled.output() else {
        return fail("the late evidence settles the send");
    };
    if !is_applied(outcome) {
        return fail("the late evidence settles it applied");
    }
    if mailer.calls() != 1 {
        return fail("settling from late evidence dispatches nothing");
    }
    Ok(())
}

/// A refused dispatch lands nothing: the step fails typed and a later run
/// dispatches again, because the receiver affirmatively holds nothing.
#[test]
fn send_retries_after_a_refused_dispatch() -> TestResult {
    let scratch = Scratch::new("outside-refused")?;
    let mailer = Arc::new(StubMailer::new());
    mailer.refuse_next_dispatch();
    let first = host_on(scratch.path(), "acme", outside_grants())?;
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    let refused = lgwks_bot::block_on(allowing(
        first.run(&sending, (Arc::clone(&mailer), b"hello".to_vec())),
    ));
    if !matches!(refused.error(), Some(FlowError::Failed { .. })) {
        return fail("a refused dispatch fails typed");
    }
    if mailer.calls() != 1 {
        return fail("the refused attempt dispatched once");
    }
    drop(first);
    let second = host_on(scratch.path(), "acme", outside_grants())?;
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    let resumed = lgwks_bot::block_on(allowing(
        second.run(&sending, (Arc::clone(&mailer), b"hello".to_vec())),
    ));
    let Some(outcome) = resumed.output() else {
        return fail("nothing landed, so the resume dispatches and settles");
    };
    if !is_applied(outcome) {
        return fail("the second attempt settles on its evidence");
    }
    if mailer.calls() != 2 {
        return fail("one dispatch per run, never a duplicate inside one");
    }
    Ok(())
}

/// A send under the default policy without a matching approval is refused
/// before anything dispatches.
#[test]
fn send_without_a_matching_approval_is_refused() -> TestResult {
    let scratch = Scratch::new("outside-gated")?;
    let mailer = Arc::new(StubMailer::new());
    let host = host_on(scratch.path(), "acme", outside_grants())?;
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    let report = lgwks_bot::block_on(host.run(&sending, (Arc::clone(&mailer), b"hello".to_vec())));
    if !matches!(report.error(), Some(FlowError::NeedsApproval { .. })) {
        return fail("an ungated send needs its approval");
    }
    if mailer.calls() != 0 {
        return fail("a refused send dispatches nothing");
    }
    Ok(())
}

/// A send with nowhere durable to record its intent is refused rather than
/// run ungated.
#[test]
fn send_without_a_store_is_refused() -> TestResult {
    let mailer = Arc::new(StubMailer::new());
    let host = Host::builder("acme")?.grants(outside_grants()).build()?;
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    let report = lgwks_bot::block_on(allowing(host.run(&sending, (mailer, b"hello".to_vec()))));
    if !matches!(report.error(), Some(FlowError::Store { .. })) {
        return fail("a storeless send is refused");
    }
    Ok(())
}

/// A run whose host grants cover none of the words is refused before its
/// first step, naming the whole shortfall rather than the first missing
/// grant.
#[test]
fn capability_shortfall_is_refused_before_the_first_step_naming_every_grant() -> TestResult {
    let scratch = Scratch::new("outside-short")?;
    let host = host_on(scratch.path(), "acme", GrantSet::empty())?;
    let mailer = Arc::new(StubMailer::new());
    let model = StubModel::from_seed(0);
    let calling = task(
        "calls",
        |scope: Scope, input: (String, StubModel, Arc<StubMailer>, Vec<u8>)| async move {
            let (url, model, mailer, payload) = input;
            calls(&scope, &url, &model, &mailer, payload).await
        },
    )?;
    let report = lgwks_bot::block_on(host.run(
        &calling,
        ("not a url".to_owned(), model, mailer, b"hello".to_vec()),
    ));
    if !matches!(report.disposition(), Disposition::Blocked) {
        return fail("a grantless run is blocked before its first step");
    }
    let Some(deficit) = report.needs() else {
        return fail("a blocked run names its shortfall");
    };
    let named = format!("{deficit}");
    for capability in ["bot.net", "bot.model", "bot.notify"] {
        if !named.contains(capability) {
            return fail("the shortfall names every missing grant");
        }
    }
    if report
        .ended_at()
        .is_some_and(|at| at.contains("fetch") || at.contains("ask") || at.contains("send"))
    {
        return fail("the refusal names the entry, not a step past it");
    }
    Ok(())
}

/// Two tenants on the same upstream: separate budgets and separate retry
/// state, so one tenant's storm never spends the other's.
///
/// Thirteen storms of two attempts spend the first tenant's whole shared
/// budget — ten of reserve and the fraction the first attempts earned — and
/// the thirteenth is throttled on its first retry; the other tenant, on its
/// own host and budget, still exhausts its own attempts instead.
#[test]
fn two_tenants_keep_separate_budgets() -> TestResult {
    let scratch_a = Scratch::new("outside-tenant-a")?;
    let scratch_b = Scratch::new("outside-tenant-b")?;
    let grants = || GrantSet::empty().grant(Cap::net());
    let host_a = host_on(scratch_a.path(), "acme", grants())?;
    let upstream = Upstream::serve(Behavior::Storm, 25, Duration::ZERO)?;
    let storm = task("storm", |scope: Scope, url: String| async move {
        let options = FetchOptions::default().attempts(2);
        get(&scope, &url, &options).await
    })?;
    let mut throttled_at = None;
    for index in 0..13 {
        let report = lgwks_bot::block_on(host_a.run(&storm, upstream.url()));
        if matches!(report.error(), Some(FlowError::Throttled { .. })) {
            throttled_at = Some(index);
            break;
        }
        if !matches!(report.error(), Some(FlowError::Exhausted { .. })) {
            upstream.join();
            return fail("every storm before the throttled one exhausts");
        }
    }
    if throttled_at != Some(12) {
        upstream.join();
        return fail("thirteen storms spend the tenant's budget");
    }
    if upstream.hits() != 25 {
        upstream.join();
        return fail("twelve storms dial twice and the throttled one once");
    }
    let host_b = host_on(scratch_b.path(), "globex", grants())?;
    let report = lgwks_bot::block_on(host_b.run(&storm, upstream.url()));
    upstream.join();
    if !matches!(report.error(), Some(FlowError::Exhausted { .. })) {
        return fail("the other tenant's budget is untouched");
    }
    Ok(())
}

/// The plan exposes every outbound reach: the map carries each outside word
/// with the capability it declares.
#[test]
fn the_plan_exposes_every_outbound_reach() -> TestResult {
    use lgwks_bot::script::StepKind;
    let mut kinds = Vec::new();
    for flow in ARCHITECTURE.flows() {
        for step in flow.steps() {
            kinds.push((step.kind(), step.subject().to_owned()));
        }
    }
    for (kind, capability) in [
        (StepKind::Fetch, "bot.net"),
        (StepKind::Ask, "bot.model"),
        (StepKind::Send, "bot.notify"),
    ] {
        if !kinds
            .iter()
            .any(|entry: &(StepKind, String)| entry.0 == kind && entry.1 == capability)
        {
            return fail("the plan names the word and its capability");
        }
    }
    Ok(())
}

/// A model that answers one JSON report, for the decode test.
struct JsonModel;

impl ModelPort for JsonModel {
    /// Never fails: the answer is fixed.
    type Error = FlowError;

    /// The fixed report.
    async fn complete(&self, _prompt: &[u8], _max_tokens: u32) -> Result<ModelOutput, FlowError> {
        Ok((br#"{"code":200}"#.to_vec(), false).into())
    }
}

/// A JSON report, as the stub model answers it.
#[derive(Debug, PartialEq, Eq, lgwks_std::json::Serialize, lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct Report {
    /// The status the model reported.
    code: u16,
}
