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

#![cfg(all(feature = "script", feature = "ephemeral"))]

use std::error::Error;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lgwks_bot::cap::Cap;
use lgwks_bot::effect::RunId;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::script::{AskRequest, FlowError, ModelOutput, ModelPort, Scope};
use lgwks_bot::task::{Host, Report, Task, task};

use crate::scratch::Scratch;
use crate::sim::Sim;

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
}

impl ReplayModel {
    /// A model for `prompt` whose flow crashes after its ask `crashes` times.
    fn new(prompt: Vec<u8>, crashes: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            prompt: Mutex::new(prompt),
            budget: AtomicU32::new(1_000),
            crashes_left: AtomicUsize::new(crashes),
            truncate_next: AtomicBool::new(false),
        })
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
