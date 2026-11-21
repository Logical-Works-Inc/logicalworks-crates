//! A model call is a durable step: a resumed run reads the answer it recorded and never pays the model twice.
//!
//! Source: the durable-execution contract Temporal (activity results in history), Restate (`ctx.run`) and DBOS
//! (step outputs) each keep for a non-deterministic call, applied to `ask`, the costliest call a flow makes.
//! Before this, `ask_in` called the model on every resume, so a crash after an answer spent the tokens again and
//! could resume on a different answer than the run already acted on.
//!
//! The oracle is the model's own call counter: the stub answers `answer <n>` for its n-th call, so a replay that
//! re-called the model returns other bytes and a higher count, whatever the decode says.
//!
//! | Test | Fool | Property it pins |
//! |---|---|---|
//! | `sim_a_resumed_ask_never_calls_the_model_twice` | pulled plug, double-clicker, giant and empty, weird bytes | one call across every crash and resume, the first answer returned |
//! | `sim_a_resume_under_another_prompt_is_refused_before_the_model` | fat finger | a changed prompt or budget is `Drift::Input`, and the model is not called |
//! | `sim_a_cut_answer_is_never_remembered` | liar upstream | a truncated answer fails typed, records nothing, and the resume asks again once |
//! | `sim_without_a_store_every_run_calls_the_model` | saboteur | no store, no durability claim: each run pays its own call |
//! | `sim_a_token_budget_stops_the_ask_before_the_model` | liar upstream, giant and empty | each ask reserves its budget first and settles to the bill; past the allowance it is refused before the model |
//! | `sim_concurrent_asks_never_spend_past_the_budget` | monkey, neighbour | asks racing on one allowance never bill past it |
//! | `sim_a_replayed_ask_spends_no_tokens` | pulled plug | a resume reads the recorded answer and charges the allowance nothing |
//!
//! The token oracle is a reference fold over the seeded plan: an ask with `m` budgeted tokens and a bill of `b` is
//! admitted while `m` tokens remain and leaves `remaining - b`, floored at zero, as Pydantic AI's `UsageLimits`
//! measures a run's spend against the provider's own count.

#![cfg(all(feature = "script", feature = "ephemeral"))]

use std::error::Error;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lgwks_bot::cap::Cap;
use lgwks_bot::effect::RunId;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::script::{
    Allowance, AskRequest, FlowError, Justify, ModelOutput, ModelPort, Rule, Scope,
};
use lgwks_bot::task::{Host, Report, Task, task};

use crate::scratch::Scratch;
use crate::sim::Sim;
#[path = "../support/yield_once.rs"]
mod yield_once;

use yield_once::YieldOnce;

/// A test's result: an error fails it with the refusal behind it.
type TestResult = Result<(), Box<dyn Error>>;

/// Seeds per sweep; each runs twice and must replay the same trace.
const SEEDS: u64 = 16;

/// One run of the drafting flow, boxed so the task over it has a nameable type.
type DraftRun = Pin<Box<dyn Future<Output = Result<Vec<u8>, FlowError>>>>;

/// The drafting flow as a task a host runs and resumes.
type DraftTask = Task<fn(Scope, Arc<ReplayModel>) -> DraftRun>;

/// A model that numbers its answers, so a second call is visible in the bytes.
struct ReplayModel {
    /// Calls answered so far.
    calls: AtomicUsize,
    /// The prompt the flow sends, changeable between a crash and its resume.
    prompt: Mutex<Vec<u8>>,
    /// The token budget the flow declares.
    budget: AtomicU32,
    /// Runs left that crash after their ask.
    crashes_left: AtomicUsize,
    /// Cut the next answer, as a provider that stopped mid-output does.
    truncate_next: AtomicBool,
    /// The token allowance the flow's asks answer to, when one is installed.
    allowance: Option<Arc<Allowance>>,
}

impl ReplayModel {
    /// A model for `prompt` whose flow crashes after its ask `crashes` times.
    fn new(prompt: Vec<u8>, crashes: usize) -> Arc<Self> {
        Arc::new(Self::unshared(prompt, crashes, None))
    }

    /// The same model, its flow's asks answering to `allowance`.
    fn metered(prompt: Vec<u8>, crashes: usize, allowance: Arc<Allowance>) -> Arc<Self> {
        Arc::new(Self::unshared(prompt, crashes, Some(allowance)))
    }

    /// The model before it is shared.
    fn unshared(prompt: Vec<u8>, crashes: usize, allowance: Option<Arc<Allowance>>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            prompt: Mutex::new(prompt),
            budget: AtomicU32::new(1_000),
            crashes_left: AtomicUsize::new(crashes),
            truncate_next: AtomicBool::new(false),
            allowance,
        }
    }

    /// Calls answered so far.
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The prompt the next run sends.
    fn prompt(&self) -> Result<Vec<u8>, FlowError> {
        match self.prompt.lock() {
            Ok(held) => Ok(held.clone()),
            Err(poisoned) => Err(FlowError::failed(format!(
                "the test prompt lock is poisoned: {poisoned}"
            ))),
        }
    }

    /// Replace the prompt the next run sends.
    fn set_prompt(&self, prompt: Vec<u8>) -> TestResult {
        match self.prompt.lock() {
            Ok(mut held) => {
                *held = prompt;
                Ok(())
            }
            Err(poisoned) => Err(format!("the test prompt lock is poisoned: {poisoned}").into()),
        }
    }

    /// Whether this run crashes after its ask, spending one crash when it does.
    fn take_crash(&self) -> bool {
        self.crashes_left
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }
}

impl ModelPort for ReplayModel {
    /// Never fails: the answer is the call number.
    type Error = FlowError;

    /// `answer <n>` for the n-th call, cut when told to.
    async fn complete(&self, _prompt: &[u8], _max_tokens: u32) -> Result<ModelOutput, FlowError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let truncated = self.truncate_next.swap(false, Ordering::SeqCst);
        Ok((format!("answer {call}").into_bytes(), truncated).into())
    }
}

/// One run: ask the model, then crash when the model says this run crashes.
fn draft_run(scope: Scope, model: Arc<ReplayModel>) -> DraftRun {
    Box::pin(async move {
        let budget = model.budget.load(Ordering::SeqCst);
        let request = AskRequest::default()
            .prompt(model.prompt()?)
            .max_tokens(budget);
        let scope = match model.allowance {
            Some(ref allowance) => scope.with_justification(token_rules(allowance)),
            None => scope,
        };
        let reply: Vec<u8> = lgwks_bot::script::ask(&scope, "draft", &*model, &request).await?;
        if model.take_crash() {
            lgwks_std::trace::debug!("draft_run: crashing after the ask, as the seed drew");
            return Err(FlowError::failed("crashed after the ask"));
        }
        Ok(reply)
    })
}

/// The drafting task.
fn drafting() -> Result<DraftTask, Box<dyn Error>> {
    let body: fn(Scope, Arc<ReplayModel>) -> DraftRun = draft_run;
    Ok(task("draft", body)?)
}

/// A host for `tenant` holding `bot.model`, over the run store in `dir` when one is given.
fn host(dir: Option<&std::path::Path>, tenant: &str) -> Result<Host, Box<dyn Error>> {
    let builder = Host::builder(tenant)?.grants(GrantSet::empty().grant(Cap::new("bot.model")));
    Ok(match dir {
        Some(dir) => builder.run_store(dir)?.build()?,
        None => builder.build()?,
    })
}

/// The run a report names, or why it kept none.
fn run_of(report: &Report<Vec<u8>>) -> Result<RunId, Box<dyn Error>> {
    report
        .run_id()
        .ok_or_else(|| "a durable run names its run".into())
}

/// A seeded prompt: empty, giant, or arbitrary bytes, NUL and invalid UTF-8 included.
fn prompt(sim: &mut Sim) -> Result<Vec<u8>, Box<dyn Error>> {
    let len = match sim.rng().below(3) {
        0 => 0,
        1 => 65_536,
        _ => usize::try_from(sim.rng().between(1, 512))?,
    };
    let mut bytes = Vec::with_capacity(len);
    for _ in 0..len {
        bytes.push(u8::try_from(sim.rng().below(256))?);
    }
    Ok(bytes)
}

/// A seeded tenant.
fn tenant(sim: &mut Sim) -> &'static str {
    if sim.rng().below(2) == 0 {
        "acme"
    } else {
        "zenith"
    }
}

/// Run `body` for every seed twice and require the same, non-empty trace.
fn replayed(mut body: impl FnMut(&mut Sim) -> TestResult) -> TestResult {
    for seed in 0..SEEDS {
        let mut first = Sim::new(seed);
        body(&mut first)?;
        let mut second = Sim::new(seed);
        body(&mut second)?;
        if first.trace.is_empty() || first.hash() != second.hash() {
            return fail(format!(
                "seed {seed}: the same seed must replay the same trace"
            ));
        }
    }
    Ok(())
}

/// Run the drafting flow once, crashing after its ask: the run it left.
fn crash_once(
    dir: &std::path::Path,
    tenant: &str,
    model: &Arc<ReplayModel>,
) -> Result<(RunId, DraftTask), Box<dyn Error>> {
    let draft = drafting()?;
    let crashed =
        lgwks_bot::script::block_on(host(Some(dir), tenant)?.run(&draft, Arc::clone(model)));
    if !matches!(crashed.error(), Some(FlowError::Failed { .. })) {
        return fail(format!(
            "the first run crashes after its ask: {:?}",
            crashed.error()
        ));
    }
    Ok((run_of(&crashed)?, draft))
}

/// Log `what` and refuse the test with it, so a failing seed leaves a signal.
fn fail<T>(what: String) -> Result<T, Box<dyn Error>> {
    lgwks_std::trace::debug!(%what, "sim_ask_replay: refusing the test");
    Err(what.into())
}

/// The rule name every token refusal carries.
const TOKEN_RULE: &str = "model-tokens";

/// One token rule over `allowance`.
fn token_rules(allowance: &Arc<Allowance>) -> Justify {
    Justify::new(vec![Rule::tokens(
        TOKEN_RULE,
        Arc::clone(allowance),
        "model-budget-spent",
    )])
}

/// A model that bills what its plan says for each call, yielding once first so racing asks interleave.
struct BilledModel {
    /// The bill for each call, in call order; past the end, the budget.
    bills: Vec<u64>,
    /// Calls answered so far.
    calls: AtomicUsize,
    /// Tokens billed so far.
    billed: AtomicU64,
}

impl BilledModel {
    /// A model billing `bills` in call order.
    fn new(bills: Vec<u64>) -> Arc<Self> {
        Arc::new(Self {
            bills,
            calls: AtomicUsize::new(0),
            billed: AtomicU64::new(0),
        })
    }
}

impl ModelPort for BilledModel {
    /// Never fails: the bill is the plan's.
    type Error = FlowError;

    /// `answer <n>`, billed as the plan says.
    async fn complete(&self, _prompt: &[u8], max_tokens: u32) -> Result<ModelOutput, FlowError> {
        let _turn = YieldOnce::new(0).await;
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let bill = match self.bills.get(call) {
            Some(&bill) => bill,
            None => u64::from(max_tokens),
        };
        self.billed.fetch_add(bill, Ordering::SeqCst);
        let output: ModelOutput = (format!("answer {call}").into_bytes(), false).into();
        Ok(output.with_billed(bill))
    }
}

/// The asks a seeded plan makes: each one's token budget and what the provider bills for it.
type Plan = Vec<(u32, u64)>;

/// One run of a seeded plan: every ask in turn under the token rule, until one is refused.
type PlanRun = Pin<Box<dyn Future<Output = Result<u64, FlowError>>>>;

/// What a plan run is handed.
type PlanInput = (Arc<BilledModel>, Arc<Allowance>, Arc<Plan>);

/// Every ask of the plan in order, each its own step; the value is the asks answered.
fn plan_run(scope: Scope, (model, allowance, plan): PlanInput) -> PlanRun {
    Box::pin(async move {
        let governed = scope.with_justification(token_rules(&allowance));
        let mut answered = 0_u64;
        for (index, &(budget, _)) in plan.iter().enumerate() {
            let request = AskRequest::default().max_tokens(budget);
            let _reply: Vec<u8> =
                lgwks_bot::script::ask(&governed, &format!("ask-{index}"), &*model, &request)
                    .await?;
            answered = answered.saturating_add(1);
        }
        Ok(answered)
    })
}

/// Every ask of the plan at once, at most `width` in flight, under one token rule.
fn racing_run(scope: Scope, (model, allowance, plan): PlanInput) -> PlanRun {
    Box::pin(async move {
        let governed = scope.with_justification(token_rules(&allowance));
        let width = NonZeroUsize::new(8).ok_or_else(|| FlowError::failed("eight is non-zero"))?;
        let replies = lgwks_bot::script::each(
            &governed,
            "asks",
            Some(width),
            plan.iter().map(|&(budget, _)| budget),
            |scope, budget| {
                let model = Arc::clone(&model);
                async move {
                    let request = AskRequest::default().max_tokens(budget);
                    lgwks_bot::script::ask::<Vec<u8>, _>(&scope, "ask", &*model, &request).await
                }
            },
        )
        .await?;
        u64::try_from(replies.len()).map_err(|error| FlowError::failed(error.to_string()))
    })
}

/// A seeded plan: 1 to 40 asks, each budgeted 1 to 400 tokens and billed 0 to 200 past it, or within it when
/// `within` is set.
fn plan(sim: &mut Sim, within: bool) -> Plan {
    let asks = sim.rng().between(1, 41);
    let mut plan = Vec::new();
    for _ in 0..asks {
        let budget = sim.rng().between(1, 401);
        let ceiling = if within {
            budget.saturating_add(1)
        } else {
            budget.saturating_add(201)
        };
        plan.push((budget, u64::from(sim.rng().below(ceiling))));
    }
    plan
}

/// The reference fold: the asks answered and the tokens left, for `plan` against `allowance` tokens.
fn expected(plan: &Plan, allowance: u64) -> (u64, u64) {
    let mut left = allowance;
    let mut answered = 0_u64;
    for &(budget, bill) in plan {
        if left < u64::from(budget) {
            break;
        }
        left = left.saturating_sub(bill);
        answered = answered.saturating_add(1);
    }
    (answered, left)
}

/// The token rule's refusal, or why the error is another one.
fn token_refusal(error: Option<&FlowError>) -> TestResult {
    let Some(error) = error else {
        return fail(String::from("a spent allowance is refused, never answered"));
    };
    match *error {
        FlowError::Justified { ref rule, .. } if rule == TOKEN_RULE => Ok(()),
        ref other => fail(format!(
            "a spent allowance is the token rule's refusal: {other:?}"
        )),
    }
}

#[test]
fn sim_a_token_budget_stops_the_ask_before_the_model() -> TestResult {
    replayed(|sim| {
        let plan = Arc::new(plan(sim, false));
        let tokens = u64::from(sim.rng().between(1, 5_001));
        let model = BilledModel::new(plan.iter().map(|&(_, bill)| bill).collect());
        let allowance = Arc::new(Allowance::new(tokens));
        let body: fn(Scope, PlanInput) -> PlanRun = plan_run;
        let report = lgwks_bot::script::block_on(host(None, tenant(sim))?.run(
            &task("plan", body)?,
            (
                Arc::clone(&model),
                Arc::clone(&allowance),
                Arc::clone(&plan),
            ),
        ));
        let (answered, left) = expected(&plan, tokens);
        assert_eq!(
            u64::try_from(model.calls.load(Ordering::SeqCst))?,
            answered,
            "the model answers exactly the asks the allowance admits, never the refused one"
        );
        assert_eq!(
            allowance.remaining(),
            left,
            "every admitted ask settles to its bill"
        );
        if u64::try_from(plan.len())? == answered {
            assert_eq!(
                report.output(),
                Some(&answered),
                "an unspent plan answers whole"
            );
        } else {
            token_refusal(report.error())?;
        }
        sim.trace.record_number("answered", answered);
        sim.trace.record_number("left", left);
        Ok(())
    })
}

#[test]
fn sim_concurrent_asks_never_spend_past_the_budget() -> TestResult {
    replayed(|sim| {
        let plan = Arc::new(plan(sim, true));
        let tokens = u64::from(sim.rng().between(1, 5_001));
        let model = BilledModel::new(plan.iter().map(|&(_, bill)| bill).collect());
        let allowance = Arc::new(Allowance::new(tokens));
        let body: fn(Scope, PlanInput) -> PlanRun = racing_run;
        let report = lgwks_bot::script::block_on(host(None, tenant(sim))?.run(
            &task("race", body)?,
            (
                Arc::clone(&model),
                Arc::clone(&allowance),
                Arc::clone(&plan),
            ),
        ));
        let billed = model.billed.load(Ordering::SeqCst);
        assert!(
            billed <= tokens,
            "asks racing on one allowance billed {billed} of {tokens}"
        );
        let unbilled = tokens.saturating_sub(billed);
        if report.output().is_some() {
            assert_eq!(
                allowance.remaining(),
                unbilled,
                "a finished race holds exactly what the bills left"
            );
        } else {
            token_refusal(report.error())?;
            assert!(
                allowance.remaining() <= unbilled,
                "an ask the refusal cancelled keeps its reservation, never refunded unseen: {} of {unbilled}",
                allowance.remaining()
            );
        }
        sim.trace.record_number("billed", billed);
        Ok(())
    })
}

#[test]
fn sim_a_replayed_ask_spends_no_tokens() -> TestResult {
    replayed(|sim| {
        let scratch = Scratch::new("ask-replay-tokens")?;
        let tenant = tenant(sim);
        let tokens = u64::from(sim.rng().between(1_000, 5_001));
        let allowance = Arc::new(Allowance::new(tokens));
        let model = ReplayModel::metered(prompt(sim)?, 1, Arc::clone(&allowance));
        let (run, draft) = crash_once(scratch.path(), tenant, &model)?;
        let after_crash = allowance.remaining();
        let resumed = lgwks_bot::script::block_on(host(Some(scratch.path()), tenant)?.resume(
            run,
            &draft,
            Arc::clone(&model),
        ));
        assert_eq!(
            resumed.output(),
            Some(&b"answer 1".to_vec()),
            "the resume replays"
        );
        assert_eq!(
            after_crash,
            tokens.saturating_sub(1_000),
            "an unbilled call keeps its whole reservation"
        );
        assert_eq!(
            allowance.remaining(),
            after_crash,
            "a replayed ask charges the allowance nothing"
        );
        sim.trace.record_number("left", after_crash);
        Ok(())
    })
}

#[test]
fn sim_a_resumed_ask_never_calls_the_model_twice() -> TestResult {
    replayed(|sim| {
        let scratch = Scratch::new("ask-replay")?;
        let tenant = tenant(sim);
        let prompt = prompt(sim)?;
        let crashes = usize::try_from(sim.rng().between(1, 4))?;
        let model = ReplayModel::new(prompt.clone(), crashes);
        let (run, draft) = crash_once(scratch.path(), tenant, &model)?;
        let mut resumes = 0_u64;
        let answer =
            loop {
                resumes = resumes.saturating_add(1);
                let resumed = lgwks_bot::script::block_on(
                    host(Some(scratch.path()), tenant)?.resume(run, &draft, Arc::clone(&model)),
                );
                if let Some(output) = resumed.output() {
                    break output.clone();
                }
                if !matches!(resumed.error(), Some(FlowError::Failed { .. })) || resumes > 8 {
                    return Err(format!(
                        "a resume only crashes again or answers: {:?}",
                        resumed.error()
                    )
                    .into());
                }
            };
        assert_eq!(
            model.calls(),
            1,
            "one model call across {crashes} crashes and {resumes} resumes"
        );
        assert_eq!(
            answer,
            b"answer 1".to_vec(),
            "the resume returns the first answer, not a new one"
        );
        sim.record(tenant);
        sim.trace
            .record_number("prompt bytes", u64::try_from(prompt.len())?);
        sim.trace.record_number("resumes", resumes);
        sim.trace
            .record_number("calls", u64::try_from(model.calls())?);
        Ok(())
    })
}

#[test]
fn sim_a_resume_under_another_prompt_is_refused_before_the_model() -> TestResult {
    replayed(|sim| {
        let scratch = Scratch::new("ask-replay-drift")?;
        let tenant = tenant(sim);
        let prompt = prompt(sim)?;
        let model = ReplayModel::new(prompt.clone(), 1);
        let (run, draft) = crash_once(scratch.path(), tenant, &model)?;
        let budget_moved = sim.rng().below(2) == 0;
        if budget_moved {
            model.budget.store(999, Ordering::SeqCst);
        } else {
            let mut edited = prompt;
            match edited.first_mut() {
                Some(first) => *first = first.wrapping_add(1),
                None => edited.push(0),
            }
            model.set_prompt(edited)?;
        }
        let resumed = lgwks_bot::script::block_on(host(Some(scratch.path()), tenant)?.resume(
            run,
            &draft,
            Arc::clone(&model),
        ));
        let Some(error) = resumed.error() else {
            return Err("a resume under another prompt is refused, never answered".into());
        };
        let kind = match *error {
            FlowError::Incompatible { ref drift, .. } => drift.kind(),
            ref other => {
                return Err(format!("another prompt is a typed drift, not {other:?}").into());
            }
        };
        assert_eq!(kind, "input", "the drift names the call's input");
        assert_eq!(model.calls(), 1, "a refused replay never calls the model");
        sim.record(if budget_moved { "budget" } else { "prompt" });
        sim.record(kind);
        Ok(())
    })
}

#[test]
fn sim_a_cut_answer_is_never_remembered() -> TestResult {
    replayed(|sim| {
        let scratch = Scratch::new("ask-replay-cut")?;
        let tenant = tenant(sim);
        let model = ReplayModel::new(prompt(sim)?, 0);
        model.truncate_next.store(true, Ordering::SeqCst);
        let draft = drafting()?;
        let cut = lgwks_bot::script::block_on(
            host(Some(scratch.path()), tenant)?.run(&draft, Arc::clone(&model)),
        );
        if !matches!(cut.error(), Some(FlowError::InvalidBound { .. })) {
            return Err(format!("a cut answer is refused as a bound: {:?}", cut.error()).into());
        }
        let resumed = lgwks_bot::script::block_on(host(Some(scratch.path()), tenant)?.resume(
            run_of(&cut)?,
            &draft,
            Arc::clone(&model),
        ));
        assert_eq!(
            resumed.output(),
            Some(&b"answer 2".to_vec()),
            "nothing was remembered, so the resume asks once more and gets the whole answer"
        );
        assert_eq!(
            model.calls(),
            2,
            "the cut call and the one resume, never a third"
        );
        sim.record(tenant);
        Ok(())
    })
}

#[test]
fn sim_without_a_store_every_run_calls_the_model() -> TestResult {
    replayed(|sim| {
        let tenant = tenant(sim);
        let runs = sim.rng().between(1, 5);
        let model = ReplayModel::new(prompt(sim)?, 0);
        let draft = drafting()?;
        let storeless = host(None, tenant)?;
        for run in 1..=runs {
            let report = lgwks_bot::script::block_on(storeless.run(&draft, Arc::clone(&model)));
            assert_eq!(
                report.output(),
                Some(&format!("answer {run}").into_bytes()),
                "a storeless run claims no record and pays its own call"
            );
        }
        assert_eq!(
            model.calls(),
            usize::try_from(runs)?,
            "one call per storeless run"
        );
        sim.trace.record_number("runs", u64::from(runs));
        Ok(())
    })
}
