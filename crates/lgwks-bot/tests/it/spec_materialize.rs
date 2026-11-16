//! `Bot::from_spec`: the materializer that walks a `BotSpec` against a registry.
//!
//! Two claims are held here, and they are the two the contract names. T25: a
//! spec materialized from JSON and a bot built natively from the same domains
//! produce the same normalized operation trace — which domains were polled and
//! which actions executed, in order — because they go through the same
//! `assemble`/`build` path with no second interpreter. T23: a spec with every
//! kind of unmet need returns them all in one attributed `NeedSet`, and nothing
//! is polled or executed on the way.
//!
//! Everything drives the shipped surface: the real `Bot`, the real
//! `DomainRegistry` built with `domains!`, the real `Source`/`Action` handles,
//! and a real `MemoryJournal` behind a real `EffectScope`. The domains here are
//! deliberately tiny and share one thread-local recorder, so the "same trace"
//! assertion compares what a person would see rather than an internal field.
//!
//! This file holds the domains, the scopes and the builders every row shares;
//! the rows live beside it by behaviour: `traces` (T25), `needs` (T23),
//! `refusals`, `concurrency` and `struct_output`.

use std::cell::RefCell;
use std::error::Error;

use lgwks_bot::broker::Broker;
use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::domains;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::effect::{EnvironmentId, FlowRevision, RunId};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::registry::{Action, Source};
use lgwks_bot::spec::{Admission, Bot, BotSpec, EffectIdentity, EffectScope};
use lgwks_bot::verb::{EffectLifetime, Evaluate, Execute, Observe};

mod concurrency;
mod needs;
mod refusals;
mod struct_output;
mod traces;

/// A test result that can carry an `Admission`, an `EcsError` or an id error.
type TestResult<T> = Result<T, Box<dyn Error>>;

/// A bot's fired count paired with the operations this thread recorded for it.
type Run = (usize, Vec<String>);

// ── The operation trace ────────────────────────────────────────────────────

thread_local! {
    /// The operations the domains performed on this thread, in order.
    ///
    /// A thread-local rather than a captured `Rc`: a registry constructor is a
    /// bare function pointer with no context, so the recorder is the only state
    /// shared between a natively constructed domain and one a registry built
    /// from the same `target`.
    static OPERATIONS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Append one operation to the current thread's trace.
pub(crate) fn record(operation: impl Into<String>) {
    OPERATIONS.with(|trace| trace.borrow_mut().push(operation.into()));
}

/// Take the trace, leaving it empty.
pub(crate) fn take_trace() -> Vec<String> {
    OPERATIONS.with(|trace| std::mem::take(&mut *trace.borrow_mut()))
}

/// Forget whatever an earlier run on this thread recorded, so a test reads only
/// its own operations.
pub(crate) fn clear_trace() {
    drop(take_trace());
}

// ── Domains ────────────────────────────────────────────────────────────────

/// Parse a target as a count, or refuse it — the malformed-input case a
/// constructor reports through admission.
///
/// The `ParseIntError` is recorded rather than dropped: `target` is untrusted
/// text, and the parse error's own message ("invalid digit found in string",
/// "cannot parse integer from empty string") is the part that says whether the
/// value was empty or merely non-numeric. `EcsError::IncompleteSpec` renders it
/// through `Escaped`, so the untrusted text it quotes cannot forge a log line.
pub(crate) fn parse_count(target: &str) -> Result<u16, EcsError> {
    target
        .parse::<u16>()
        .map_err(|not_a_count| EcsError::IncompleteSpec {
            field: "target",
            cause: not_a_count.to_string(),
        })
}

/// A source that reports the count its `target` parsed to.
///
/// Shared with `sim_spec_materialize`, whose sweep admits specs over the same
/// source rather than a copy of it.
pub(crate) struct Counter {
    /// The value to report.
    value: u16,
    /// The capabilities this instance requires.
    caps: Vec<Cap>,
}

impl Counter {
    /// Build a cap-free one directly, for a native bot.
    fn new(value: u16) -> Self {
        Self::with_caps(value, Vec::new())
    }

    /// Build one with the given capabilities directly, for a native bot.
    fn with_caps(value: u16, caps: Vec<Cap>) -> Self {
        Self { value, caps }
    }

    /// Build a cap-free one from the `target` its spec names.
    pub(crate) fn from_target(target: &str) -> Result<Source, EcsError> {
        Ok(Source::ordered(Self::with_caps(
            parse_count(target)?,
            Vec::new(),
        )))
    }

    /// Build one requiring `bot.net` from the `target` its spec names.
    pub(crate) fn net_from_target(target: &str) -> Result<Source, EcsError> {
        Ok(Source::ordered(Self::with_caps(
            parse_count(target)?,
            vec![Cap::net()],
        )))
    }
}

impl Observe for Counter {
    type Output = u16;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u16, EcsError> {
        call.0.check(&self.caps)?;
        record(format!("poll:{}", self.domain_id()));
        Ok(self.value)
    }

    fn domain_id(&self) -> &str {
        "test::counter"
    }
}

/// An action that records the value it was handed, tagged by its target.
///
/// Shared with `script_domains`, whose flows act through the same action a
/// spec's chain does.
pub(crate) struct Page {
    /// A label the target carried, so two pages in a chain are distinguishable.
    label: String,
    /// The capabilities this instance requires.
    caps: Vec<Cap>,
}

impl Page {
    /// Build a cap-free one directly, for a native bot.
    fn new(label: &str) -> Self {
        Self::with_caps(label, Vec::new())
    }

    /// Build one with the given capabilities directly, for a native bot.
    fn with_caps(label: &str, caps: Vec<Cap>) -> Self {
        Self {
            label: label.to_owned(),
            caps,
        }
    }

    /// Build a cap-free one from the `target` its spec names.
    pub(crate) fn from_target(target: &str) -> Result<Action, EcsError> {
        Ok(Action::new(Self::with_caps(target, Vec::new())))
    }

    /// Build one requiring `bot.net` from the `target` its spec names.
    fn net_from_target(target: &str) -> Result<Action, EcsError> {
        Ok(Action::new(Self::with_caps(target, vec![Cap::net()])))
    }
}

impl Execute for Page {
    type Input = u16;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u16)) -> Result<(), EcsError> {
        call.0.check(&self.caps)?;
        record(format!(
            "execute:{}:{}:{}",
            self.domain_id(),
            self.label,
            call.1
        ));
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::page"
    }
}

domains! {
    /// The domains these tests run: two sources and two actions, one of each
    /// requiring `bot.net`.
    pub SPEC_DOMAINS {
        observe {
            "test::counter" => Counter::from_target,
            "test::net_counter" => Counter::net_from_target,
        }
        execute {
            "test::page" => Page::from_target,
            "test::net_page" => Page::net_from_target,
        }
    }
}

domains! {
    /// A deliberately duplicated registry: two source constructors share one
    /// identifier, so admission must refuse it rather than let declaration order
    /// pick a winner.
    pub DUPLICATE_DOMAINS {
        observe {
            "test::counter" => Counter::from_target,
            "test::counter" => Counter::net_from_target,
        }
        execute {}
    }
}

// ── Scopes ─────────────────────────────────────────────────────────────────

/// The flow revision every test scope uses.
const FLOW: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// A fresh effect scope over an in-memory journal.
fn scope() -> TestResult<EffectScope> {
    scope_for_run("0102030405060708090a0b0c0d0e0f10")
}

/// A fresh effect scope whose run is named by `run_hex`.
fn scope_for_run(run_hex: &str) -> TestResult<EffectScope> {
    let environment = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    let mut broker = Broker::new();
    broker.register(environment)?;
    Ok(EffectScope::new(
        EffectIdentity::new(
            RunId::from_hex(run_hex)?,
            environment,
            FlowRevision::from_tagged("blake3_256", FLOW)?,
        ),
        broker,
        Box::new(MemoryJournal::new()),
    ))
}

// ── Materializing, tick, and native builders ───────────────────────────────

/// Materialize a spec from JSON against [`SPEC_DOMAINS`] under `grants`.
fn materialize(json: &str, grants: &GrantSet) -> TestResult<Bot> {
    let spec = BotSpec::from_json(json)?;
    Ok(Bot::from_spec(&spec, &SPEC_DOMAINS, grants, scope()?)?)
}

/// Materialize under `grants`, returning the admission refusal unopened so a
/// test can inspect the `NeedSet`.
fn admit(json: &str, grants: &GrantSet, effects: EffectScope) -> Result<Bot, Admission> {
    match BotSpec::from_json(json) {
        Ok(spec) => Bot::from_spec(&spec, &SPEC_DOMAINS, grants, effects),
        Err(cause) => Err(Admission::Refused(cause)),
    }
}

/// Tick a bot once and return the fired count together with this thread's trace.
fn tick_and_trace(bot: &mut Bot) -> TestResult<Run> {
    clear_trace();
    let fired = bot.tick()?;
    Ok((fired, take_trace()))
}

/// A native bot over one `test::counter` chain.
fn native_one<C>(value: u16, condition: C, action: Page) -> TestResult<Bot>
where
    C: Evaluate<u16> + 'static,
{
    Ok(Bot::builder("native")
        .observe(Counter::new(value))
        .on(condition, action)
        .with_effects(scope()?)
        .build(&GrantSet::empty())?)
}

/// A native bot over one chain with two always-firing entries.
fn native_two_entries(first: Page, second: Page) -> TestResult<Bot> {
    Ok(Bot::builder("native")
        .observe(Counter::new(3))
        .on(|_: &u16| true, first)
        .on(|_: &u16| true, second)
        .with_effects(scope()?)
        .build(&GrantSet::empty())?)
}

/// A native bot over two always-firing chains.
fn native_two_chains(first: Page, second: Page) -> TestResult<Bot> {
    Ok(Bot::builder("native")
        .observe(Counter::new(3))
        .on(|_: &u16| true, first)
        .observe(Counter::new(4))
        .on(|_: &u16| true, second)
        .with_effects(scope()?)
        .build(&GrantSet::empty())?)
}

/// Tick `native` once and the JSON spec once, under no grants.
fn both_runs(native: Bot, json: &str) -> TestResult<(Run, Run)> {
    both_runs_with(native, json, &GrantSet::empty())
}

/// Tick `native` once and the JSON spec once under `grants`, returning both
/// traces.
fn both_runs_with(native: Bot, json: &str, grants: &GrantSet) -> TestResult<(Run, Run)> {
    let mut native = native;
    let native_run = tick_and_trace(&mut native)?;
    let mut made = materialize(json, grants)?;
    let made_run = tick_and_trace(&mut made)?;
    Ok((native_run, made_run))
}
