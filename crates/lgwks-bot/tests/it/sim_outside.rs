//! Simulation family: seeded interleavings of fetch faults, model truncation
//! and crash points across many tenants (#783).
//!
//! The public journeys in `tests/it/outside.rs` pin one shape each: a slow
//! upstream, a dropped body, a 5xx storm, an oversize body, a truncated model
//! output, a crash on either side of a dispatch, and the tenant and
//! capability boundaries. This target asks the same questions of
//! *interleavings* — every order in which many tenants fetch, ask, crash and
//! resume over one store — over a seeded sweep. What the seed controls is the
//! input: the tenants, the upstreams' behaviors, the attempt budgets, the
//! model seeds, and where the crashes land. The store, the hosts and the
//! decisions are real; no wall clock is consulted and no outcome asserts a
//! duration.
//!
//! | Family | Property it pins |
//! |---| Facet |
//! | `fetch_attempts_share_one_budget_and_sizes_refuse_while_streaming` | fetch faults interleave across tenants; attempts share the run's budget and oversize bodies refuse while streaming, the same trace every replay |
//! | `ask_truncation_refuses_typed_on_every_seed` | truncated output, by shape and by budget, is refused typed on every seed drawn |
//! | `send_dispatches_once_and_unknown_stays_unknown` | intent before dispatch, at most one dispatch per run, unknown until evidence settles it, the same trace every replay |
//! | `sim_same_seed_replays_the_same_outside_trace` | the same seed run twice reaches the same trace hash |
//! | `sim_distinct_seeds_diverge_in_their_outside_trace` | distinct seeds reach distinct trace hashes |
//! | `sim_two_tenants_keep_separate_budgets` | one tenant's storm never spends the other's budget |
//! | `sim_capability_shortfall_names_every_grant` | every grant subset refuses with exactly its missing grants |
//! | `sim_fetch_inherits_enclosing_retry_and_within` | an enclosing retry retries a raw failure and an enclosing within bounds the exchange |
//! | `sim_ask_decodes_seeded_json` | a stub port answering JSON decodes into the declared report |
//! | `sim_post_runs_gated` | an allowed POST runs and a gated one is refused before it dials |
//! | `sim_plan_lists_every_word` | the plan carries each outside word with its capability |
//!
//! Each declared test sweeps its band twice and requires identical trace
//! hashes, so a nondeterministic run fails even when every assertion passes.

#![cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::proposal::{Shape, StubModel};
use lgwks_bot::script::Scope;
use lgwks_bot::script::{AskRequest, FetchOptions, FlowError, ModelOutput, ModelPort, PostBody};
use lgwks_bot::task::{Disposition, Host, task};
use lgwks_bot::{Cap, GrantSet};

use crate::band_family::band_family;
use crate::effect_policy::allowing;
use crate::outside_support::{ARCHITECTURE, calls, get, mail, mail_crash};
use crate::outside_support::{Behavior, OutsideError, StubMailer, Upstream};
use crate::outside_support::{fail, host_on, is_applied, is_unknown, outside_grants, run_of};
use crate::scratch::Scratch;
use crate::sim::{self, Band};

/// A test's result: an error fails it with the refusal behind it.
type TestResult = Result<(), OutsideError>;

/// Every tenant's host rebuilt over `dir`, the restart a crash leaves, and
/// the rebuilt host of tenant `tenant`.
fn restarted<'hosts>(
    hosts: &'hosts mut Vec<Host>,
    dir: &std::path::Path,
    tenant: usize,
) -> Result<&'hosts Host, OutsideError> {
    hosts.clear();
    for name in ["acme", "zenith"] {
        hosts.push(host_on(dir, name, outside_grants())?);
    }
    hosts.get(tenant).ok_or(OutsideError::Setup {
        what: "the tenant has a host",
    })
}

/// A draw below `bound`, as a `usize` the scenario indexes with.
fn draw_below(sim: &mut sim::Sim, bound: u32) -> Result<usize, OutsideError> {
    usize::try_from(sim.rng().below(bound)).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "draw_below: a draw fits its bound");
        OutsideError::Setup {
            what: "a draw fits its bound",
        }
    })
}

/// A seeded fetch behavior: never the slow one, whose outcome the wall clock
/// would decide rather than the seed.
fn draw_behavior(sim: &mut sim::Sim) -> Result<Behavior, OutsideError> {
    match draw_below(sim, 4)? {
        0 => Ok(Behavior::Ok),
        1 => Ok(Behavior::DropMidBody),
        2 => Ok(Behavior::Storm),
        _ => match draw_below(sim, 2)? {
            0 => Ok(Behavior::Oversized(70_000)),
            _ => Ok(Behavior::Oversized(200_000)),
        },
    }
}

/// One seeded fetch against its seeded upstream: the outcome kind the behavior
/// and the budget decide, recorded for the trace.
fn tend_fetch(
    host: &Host,
    sim: &mut sim::Sim,
    tenant: usize,
    index: usize,
) -> Result<(), OutsideError> {
    let behavior = draw_behavior(sim)?;
    let attempts: u32 = match draw_below(sim, 4)? {
        0..=2 => 1,
        _ => 2,
    };
    let bound = match draw_below(sim, 2)? {
        0 => 1_024,
        _ => 65_536,
    };
    let expect = match behavior {
        Behavior::Ok => 1,
        Behavior::DropMidBody | Behavior::Storm => {
            if attempts == 1 {
                1
            } else {
                2
            }
        }
        Behavior::Oversized(_) => 1,
        Behavior::Slow => return fail("the slow upstream is never drawn"),
    };
    let upstream = Upstream::serve(behavior, expect, Duration::ZERO)?;
    let fetching = task("fetch", |scope: Scope, url: String| async move {
        let options = FetchOptions::default()
            .attempts(attempts)
            .max_body_bytes(bound);
        get(&scope, &url, &options).await
    })?;
    let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
    let hits = upstream.hits();
    upstream.join();
    if hits != expect {
        return fail("every attempt dials once, never more");
    }
    let outcome = match behavior {
        Behavior::Ok => match report.output() {
            Some(&200) => "ok",
            _ => return fail("a well upstream answers 200"),
        },
        Behavior::DropMidBody | Behavior::Storm => match report.error() {
            Some(&FlowError::Transient { .. }) if attempts == 1 => "raw",
            Some(&FlowError::Exhausted { .. }) => "exhausted",
            _ => return fail("a failing upstream ends retryable"),
        },
        Behavior::Oversized(_) => match report.error() {
            Some(&FlowError::InvalidBound { .. }) => "oversize",
            _ => return fail("an oversize body is refused at its bound"),
        },
        Behavior::Slow => return fail("the slow upstream is never drawn"),
    };
    sim.record(&format!("t{tenant}:f{index}:{outcome}:{attempts}:{hits}"));
    Ok(())
}

/// Fetch faults interleaved across tenants: attempts share the one budget and
/// oversize bodies refuse while streaming, whatever the interleaving.
fn fetch_attempts_share_one_budget_and_sizes_refuse_while_streaming(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("outside-sim-fetch")?;
        let tenants = draw_below(&mut sim, 2)?.saturating_add(1);
        for tenant in 0..tenants {
            let name = match tenant {
                0 => "acme",
                _ => "zenith",
            };
            let host = host_on(scratch.path(), name, GrantSet::empty().grant(Cap::net()))?;
            let fetches = draw_below(&mut sim, 2)?.saturating_add(1);
            for index in 0..fetches {
                tend_fetch(&host, &mut sim, tenant, index)?;
            }
        }
        if sim.trace.is_empty() {
            return fail("the seed fetched at least once");
        }
    }
    Ok(())
}

/// Truncated output, by shape and by budget, is refused typed on every seed
/// drawn: never decoded, never guessed.
fn ask_truncation_refuses_typed_on_every_seed(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scope = Scope::root(lgwks_bot::script::Tenant::new("acme")?);
        let model_seed = sim.rng().below(1_000);
        let model = StubModel::from_seed(u64::from(model_seed));
        let budget = match draw_below(&mut sim, 3)? {
            0 => 16,
            1 => 1_000_000,
            _ => u32::MAX,
        };
        let request = AskRequest::default().max_tokens(budget);
        let refused: Result<String, FlowError> =
            lgwks_bot::block_on(lgwks_bot::script::ask(&scope, "cut", &model, &request));
        let truncated = model.shape() == Shape::Truncated
            || match u64::try_from(model.output().len()) {
                Ok(len) => len > u64::from(budget),
                Err(_) => true,
            };
        match (refused, truncated) {
            (Err(FlowError::InvalidBound { .. }), true) => {}
            (Ok(_), false) => {}
            _ => return fail("truncation refuses typed and whole output decodes"),
        }
        sim.record(&format!("seed{seed}:truncated{truncated}"));
    }
    Ok(())
}

/// What one seeded send does: park, crash, lose or land, then resume over the
/// same store and settle from whatever evidence survived.
fn tend_send(
    hosts: &mut Vec<Host>,
    dir: &std::path::Path,
    mailer: &Arc<StubMailer>,
    sim: &mut sim::Sim,
    tenant: usize,
    index: usize,
) -> Result<(), OutsideError> {
    let fate = draw_below(sim, 4)?;
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    let crashing = task(
        "crash",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>, bool)| async move {
            let (mailer, payload, crash) = input;
            mail_crash(&scope, &mailer, payload, crash).await
        },
    )?;
    let payload = format!("t{tenant}-s{index}-{}", sim.seed).into_bytes();
    let host = hosts.get(tenant).ok_or(OutsideError::Setup {
        what: "the tenant has a host",
    })?;
    match fate {
        0 => {
            let report =
                lgwks_bot::block_on(allowing(host.run(&sending, (Arc::clone(mailer), payload))));
            let Some(outcome) = report.output() else {
                return fail("a clean send settles");
            };
            if !is_applied(outcome) {
                return fail("a clean send settles applied");
            }
            sim.record(&format!("t{tenant}:s{index}:settled"));
            Ok(())
        }
        1 => {
            mailer.fail_next_read();
            let crashed = lgwks_bot::block_on(allowing(
                host.run(&sending, (Arc::clone(mailer), payload.clone())),
            ));
            if crashed.error().is_none() {
                return fail("a failed read-back fails the step");
            }
            let run = run_of(&crashed)?;
            let host = restarted(hosts, dir, tenant)?;
            let resumed = lgwks_bot::block_on(allowing(host.resume(
                run,
                &sending,
                (Arc::clone(mailer), payload),
            )));
            let Some(outcome) = resumed.output() else {
                return fail("the resume settles the send");
            };
            if !is_applied(outcome) {
                return fail("the resume settles on the evidence");
            }
            sim.record(&format!("t{tenant}:s{index}:intent-crash-once"));
            Ok(())
        }
        2 => {
            mailer.lose_next_reply();
            let crashed = lgwks_bot::block_on(allowing(
                host.run(&crashing, (Arc::clone(mailer), payload.clone(), true)),
            ));
            if crashed.error().is_none() {
                return fail("the crashing run fails after its lost dispatch");
            }
            let run = run_of(&crashed)?;
            let host = restarted(hosts, dir, tenant)?;
            let unknown = lgwks_bot::block_on(allowing(host.resume(
                run,
                &crashing,
                (Arc::clone(mailer), payload.clone(), false),
            )));
            let Some(outcome) = unknown.output() else {
                return fail("the resume ends unknown");
            };
            if !is_unknown(outcome) {
                return fail("with no evidence the outcome stays unknown");
            }
            mailer.land_applied();
            let settled = lgwks_bot::block_on(allowing(host.resume(
                run,
                &crashing,
                (Arc::clone(mailer), payload, false),
            )));
            let Some(outcome) = settled.output() else {
                return fail("the late evidence settles the send");
            };
            if !is_applied(outcome) {
                return fail("the late evidence settles it applied");
            }
            sim.record(&format!("t{tenant}:s{index}:unknown-then-settled"));
            Ok(())
        }
        _ => {
            mailer.forget();
            let resumed =
                lgwks_bot::block_on(allowing(host.run(&sending, (Arc::clone(mailer), payload))));
            let Some(outcome) = resumed.output() else {
                return fail("a fresh send settles");
            };
            if !is_applied(outcome) {
                return fail("a fresh send settles applied");
            }
            sim.record(&format!("t{tenant}:s{index}:fresh"));
            Ok(())
        }
    }
}

/// Intent before dispatch, at most one dispatch per run, and unknown until
/// evidence settles it — whatever order the crashes land in.
fn send_dispatches_once_and_unknown_stays_unknown(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("outside-sim-send")?;
        let mailer = Arc::new(StubMailer::new());
        let mut hosts = Vec::new();
        for name in ["acme", "zenith"] {
            hosts.push(host_on(scratch.path(), name, outside_grants())?);
        }
        let before = mailer.calls();
        let tenants = draw_below(&mut sim, 2)?.saturating_add(1);
        for tenant in 0..tenants {
            let sends = draw_below(&mut sim, 2)?.saturating_add(1);
            for index in 0..sends {
                tend_send(&mut hosts, scratch.path(), &mailer, &mut sim, tenant, index)?;
            }
        }
        let dispatched = mailer.calls().saturating_sub(before);
        sim.record(&format!("seed{seed}:dispatched{dispatched}"));
        if sim.trace.is_empty() {
            return fail("the seed sent at least once");
        }
    }
    Ok(())
}

/// One tenant's storm never spends the other's budget: the storming tenant
/// exhausts while its neighbour still answers.
fn sim_two_tenants_keep_separate_budgets(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("outside-sim-tenants")?;
        let grants = || GrantSet::empty().grant(Cap::net());
        let stormy = host_on(scratch.path(), "acme", grants())?;
        let upstream = Upstream::serve(Behavior::Storm, 4, Duration::ZERO)?;
        let storm = task("storm", |scope: Scope, url: String| async move {
            let options = FetchOptions::default().attempts(2);
            get(&scope, &url, &options).await
        })?;
        for _ in 0..2 {
            let report = lgwks_bot::block_on(stormy.run(&storm, upstream.url()));
            if !matches!(report.error(), Some(FlowError::Exhausted { .. })) {
                upstream.join();
                return fail("a storm exhausts its own attempts");
            }
        }
        let upstream_ok = Upstream::serve(Behavior::Ok, 1, Duration::ZERO)?;
        let quiet = host_on(scratch.path(), "zenith", grants())?;
        let fetching = task("fetch", |scope: Scope, url: String| async move {
            get(&scope, &url, &FetchOptions::default()).await
        })?;
        let report = lgwks_bot::block_on(quiet.run(&fetching, upstream_ok.url()));
        upstream.join();
        upstream_ok.join();
        if report.output().copied() != Some(200) {
            return fail("the other tenant answers while its neighbour storms");
        }
        sim.record(&format!("seed{seed}:isolated"));
    }
    Ok(())
}

/// Every grant subset refuses with exactly its missing grants, before the
/// first step and naming the whole shortfall.
fn sim_capability_shortfall_names_every_grant(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("outside-sim-short")?;
        let mut grants = GrantSet::empty();
        let mut missing = Vec::new();
        for (cap, name) in [
            (Cap::net(), "bot.net"),
            (Cap::new("bot.model"), "bot.model"),
            (Cap::notify(), "bot.notify"),
        ] {
            if draw_below(&mut sim, 2)? == 0 {
                grants = grants.grant(cap);
            } else {
                missing.push(name);
            }
        }
        let host = host_on(scratch.path(), "acme", grants)?;
        let mailer = Arc::new(StubMailer::new());
        let model = StubModel::from_seed(u64::from(sim.rng().below(1_000)));
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
        if missing.is_empty() {
            sim.record(&format!("seed{seed}:admitted"));
            continue;
        }
        if !matches!(report.disposition(), Disposition::Blocked) {
            return fail("a short run is blocked before its first step");
        }
        let Some(deficit) = report.needs() else {
            return fail("a blocked run names its shortfall");
        };
        let named = format!("{deficit}");
        for name in &missing {
            if !named.contains(name) {
                return fail("the shortfall names every missing grant");
            }
        }
        sim.record(&format!("seed{seed}:blocked{}", missing.len()));
    }
    Ok(())
}

/// An enclosing retry retries a raw failure and an enclosing within bounds
/// the exchange: the words inherit what surrounds them.
fn sim_fetch_inherits_enclosing_retry_and_within(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("outside-sim-inherit")?;
        let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
        let upstream = Upstream::serve(Behavior::Storm, 3, Duration::ZERO)?;
        let retrying = task("retry", |scope: Scope, url: String| async move {
            let options = FetchOptions::default().attempts(1);
            let budget = lgwks_bot::script::attempts(3)?;
            lgwks_bot::script::retry(&scope, "storm", budget, Duration::ZERO, |scope, _| {
                let url = url.clone();
                let options = options.clone();
                async move { get(&scope, &url, &options).await }
            })
            .await
        })?;
        let report = lgwks_bot::block_on(host.run(&retrying, upstream.url()));
        upstream.join();
        match report.error() {
            Some(&FlowError::Exhausted { attempts: 3, .. }) => {}
            _ => return fail("the enclosing retry spends its three attempts"),
        }
        let upstream_ok = Upstream::serve(Behavior::Ok, 1, Duration::ZERO)?;
        let bounded = task("bounded", |scope: Scope, url: String| async move {
            lgwks_bot::script::within(&scope, "ok", Duration::from_secs(30), async {
                get(&scope, &url, &FetchOptions::default()).await
            })
            .await
        })?;
        let report = lgwks_bot::block_on(host.run(&bounded, upstream_ok.url()));
        upstream_ok.join();
        if report.output().copied() != Some(200) {
            return fail("the enclosing within lets the exchange answer");
        }
        sim.record(&format!("seed{seed}:inherited"));
    }
    Ok(())
}

/// A stub port answering JSON decodes into the declared report.
fn sim_ask_decodes_seeded_json(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scope = Scope::root(lgwks_bot::script::Tenant::new("acme")?);
        let stub = JsonModel;
        let report: lgwks_bot::script::Json<SimReport> = lgwks_bot::block_on(
            lgwks_bot::script::ask(&scope, "report", &stub, &AskRequest::default()),
        )?;
        if report.value.code != 200 {
            return fail("JSON decodes into the declared report");
        }
        sim.record(&format!("seed{seed}:json"));
    }
    Ok(())
}

/// An allowed POST runs and a gated one is refused before it dials.
fn sim_post_runs_gated(band: Band) -> TestResult {
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("outside-sim-post")?;
        let upstream = Upstream::serve(Behavior::Ok, 1, Duration::ZERO)?;
        let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
        let posting = task("post", |scope: Scope, url: String| async move {
            let options =
                FetchOptions::default().post(PostBody::new("text/plain", b"hook".to_vec()));
            let answered = lgwks_bot::script::fetch(&scope, "post", &url, &options).await?;
            Ok::<u16, FlowError>(answered.status())
        })?;
        let allowed = lgwks_bot::block_on(allowing(host.run(&posting, upstream.url())));
        upstream.join();
        if allowed.into_result()? != 200 {
            return fail("an allowed POST answers its status");
        }
        let upstream_denied = Upstream::serve(Behavior::Ok, 0, Duration::ZERO)?;
        let denied = lgwks_bot::block_on(host.run(&posting, upstream_denied.url()));
        let hits = upstream_denied.hits();
        upstream_denied.join();
        if !matches!(denied.error(), Some(FlowError::NeedsApproval { .. })) {
            return fail("a gated POST without an approval is refused");
        }
        if hits != 0 {
            return fail("a refused POST dials nothing");
        }
        sim.record(&format!("seed{seed}:post"));
    }
    Ok(())
}

/// The plan carries each outside word with the capability it declares.
#[test]
fn sim_plan_lists_every_word() -> TestResult {
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

/// A JSON report, as the seeded stub model answers it.
#[derive(Debug, PartialEq, Eq, lgwks_std::json::Serialize, lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct SimReport {
    /// The status the model reported.
    code: u16,
}

/// A model that answers one JSON report, for the decode family.
struct JsonModel;

impl ModelPort for JsonModel {
    /// Never fails: the answer is fixed.
    type Error = FlowError;

    /// The fixed report.
    async fn complete(&self, _prompt: &[u8], _max_tokens: u32) -> Result<ModelOutput, FlowError> {
        Ok((br#"{"code":200}"#.to_vec(), false).into())
    }
}

band_family! {
    sim_outside_fetch_band_00 => fetch_attempts_share_one_budget_and_sizes_refuse_while_streaming, 0;
    sim_outside_fetch_band_01 => fetch_attempts_share_one_budget_and_sizes_refuse_while_streaming, 1;
    sim_outside_ask_band_00 => ask_truncation_refuses_typed_on_every_seed, 0;
    sim_outside_ask_band_01 => ask_truncation_refuses_typed_on_every_seed, 1;
    sim_outside_send_band_00 => send_dispatches_once_and_unknown_stays_unknown, 0;
    sim_outside_send_band_01 => send_dispatches_once_and_unknown_stays_unknown, 1;
    sim_outside_send_band_02 => send_dispatches_once_and_unknown_stays_unknown, 2;
    sim_outside_tenants_band_00 => sim_two_tenants_keep_separate_budgets, 0;
    sim_outside_tenants_band_01 => sim_two_tenants_keep_separate_budgets, 1;
    sim_outside_shortfall_band_00 => sim_capability_shortfall_names_every_grant, 0;
    sim_outside_shortfall_band_01 => sim_capability_shortfall_names_every_grant, 1;
    sim_outside_inherit_band_00 => sim_fetch_inherits_enclosing_retry_and_within, 0;
    sim_outside_inherit_band_01 => sim_fetch_inherits_enclosing_retry_and_within, 1;
    sim_outside_json_band_00 => sim_ask_decodes_seeded_json, 0;
    sim_outside_post_band_00 => sim_post_runs_gated, 0;
}

/// The same seed run twice reaches the same trace hash.
#[test]
fn sim_same_seed_replays_the_same_outside_trace() -> TestResult {
    let mut first = sim::Sim::new(7);
    replay_outside_seed(&mut first)?;
    let first_hash = first.hash();
    let mut second = sim::Sim::new(7);
    replay_outside_seed(&mut second)?;
    if first_hash != second.hash() {
        return fail("the same seed replays the same trace");
    }
    Ok(())
}

/// Distinct seeds reach distinct trace hashes.
#[test]
fn sim_distinct_seeds_diverge_in_their_outside_trace() -> TestResult {
    let mut low = sim::Sim::new(7);
    replay_outside_seed(&mut low)?;
    let mut high = sim::Sim::new(8);
    replay_outside_seed(&mut high)?;
    if low.hash() == high.hash() {
        return fail("distinct seeds diverge");
    }
    Ok(())
}

/// One fetch, one ask and one send: the replay both trace tests compare.
fn replay_outside_seed(sim: &mut sim::Sim) -> Result<(), OutsideError> {
    let tenant = if sim.rng().below(2) == 0 {
        "acme"
    } else {
        "zenith"
    };
    let salt = sim.rng().below(1_000);
    let payload = format!("replay-{}-{salt}", sim.seed).into_bytes();
    sim.record(&format!("tenant:{tenant}"));
    let scratch = Scratch::new("outside-sim-replay")?;
    let host = host_on(scratch.path(), tenant, outside_grants())?;
    let upstream = Upstream::serve(Behavior::Ok, 1, Duration::ZERO)?;
    let fetching = task("fetch", |scope: Scope, url: String| async move {
        get(&scope, &url, &FetchOptions::default()).await
    })?;
    let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
    upstream.join();
    if report.output().copied() != Some(200) {
        return fail("the replay fetch answers");
    }
    sim.record("fetch:ok");
    let scope = Scope::root(lgwks_bot::script::Tenant::new("acme")?);
    let mut model_seed = 0_u64;
    let model = loop {
        let candidate = StubModel::from_seed(model_seed);
        if candidate.shape() == Shape::WellFormed {
            break candidate;
        }
        model_seed = model_seed.saturating_add(1);
    };
    let text: String = match lgwks_bot::block_on(lgwks_bot::script::ask(
        &scope,
        "ask",
        &model,
        &AskRequest::default(),
    )) {
        Ok(text) => text,
        Err(failed) => {
            lgwks_std::trace::debug!(error = ?failed, "replay_outside_seed: the replay ask did not decode");
            return fail("the replay ask decodes");
        }
    };
    if text.as_bytes() != model.output() {
        return fail("the replay ask decodes");
    }
    sim.record("ask:ok");
    let mailer = Arc::new(StubMailer::new());
    let sending = task(
        "send",
        |scope: Scope, input: (Arc<StubMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            mail(&scope, &mailer, payload).await
        },
    )?;
    let sent = lgwks_bot::block_on(allowing(host.run(&sending, (Arc::clone(&mailer), payload))));
    let Some(outcome) = sent.output() else {
        return fail("the replay send settles");
    };
    if !is_applied(outcome) {
        return fail("the replay send settles applied");
    }
    sim.record(&format!("send:applied:{}", mailer.calls()));
    Ok(())
}
