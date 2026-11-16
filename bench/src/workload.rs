//! The workload both engines run, plus the builders and tick loops every
//! measurement shares.
//!
//! The timing rows, the allocation window, the profile and the probes all build
//! their bots here and advance them through the same loops, so a number from one
//! of them is a number about the same work as a number from another. The bot's
//! three verbs (`Observe`, `Execute`, `Evaluate`) and the hand-rolled baseline
//! that mirrors them live together for the same reason: a baseline defined away
//! from the workload it mirrors is a baseline that can drift from it.

use std::cell::Cell;
use std::rc::Rc;

use lgwks_bot::broker::Broker;
use lgwks_bot::effect::{EnvironmentId, FlowRevision, RunId};
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::spec::{EffectIdentity, EffectScope};
use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::spec::Bot;
use lgwks_bot::verb::{EffectLifetime, Evaluate, Execute, Observe};

use crate::error::BenchError;
use crate::scenarios::Scenario;

// ── The workload ─────────────────────────────────────────────────────────────

/// The tick counter every source derives its value from.
///
/// Shared rather than owned so all sources advance together: the value a source
/// reports is `tick / period`, which makes the whole input sequence a pure
/// function of the tick index.
#[derive(Clone)]
pub(super) struct Clock(Rc<Cell<u64>>);

impl Clock {
    pub(super) fn new() -> Self {
        Self(Rc::new(Cell::new(0)))
    }

    pub(super) fn set(&self, tick: u64) {
        self.0.set(tick);
    }
}

/// A source whose value steps once every `period` ticks.
///
/// `period == 1` changes every tick (maximum churn); `period == 100` changes one
/// tick in a hundred (steady state). The change *rate* is therefore a knob on
/// the scenario rather than a property of a random seed.
struct Source {
    clock: Clock,
    period: u64,
}

impl Observe for Source {
    type Output = u64;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, _call: (Auth, ())) -> Result<u64, EcsError> {
        let period = if self.period == 0 { 1 } else { self.period };
        Ok(self.clock.0.get() / period)
    }

    /// The value, as a revision.
    ///
    /// This is the change-tick path: the substrate compares this against the
    /// revision it last committed for the chain and **skips the poll entirely**
    /// when they match, so an unchanged tick costs one integer comparison per
    /// source and allocates nothing. A real source would report something its
    /// world maintains for it — an ETag, an `mtime`, a subscription sequence — and
    /// this one reports the counter it already holds, which is the same contract
    /// with a cheaper witness.
    ///
    /// Reporting the value is honest here and is not the same as the digest the
    /// rig used to implement: the deprecated `fingerprint` was read *beside* the
    /// poll and could describe a value the poll never returned, whereas this is
    /// read before the poll it may skip, so it can only ever describe the value
    /// the substrate commits (`docs/bot-on-ecs.md` §4).
    fn revision(&self) -> Option<u64> {
        let period = if self.period == 0 { 1 } else { self.period };
        Some(self.clock.0.get() / period)
    }

    fn domain_id(&self) -> &'static str {
        "bench::source"
    }
}

/// The effect: record that it happened.
///
/// Deliberately trivial. The question this rig asks is what the *schedule* costs,
/// so the effect must not dominate it. A heavier action would measure the
/// action, and every engine would converge to that cost.
struct Tally(Rc<Cell<u64>>);

impl Execute for Tally {
    type Input = u64;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    /// Local, and stated here rather than left to the default.
    ///
    /// The default is [`EffectLifetime::External`], which the crate answers with a
    /// durability admission: an external handoff must not leave on a record that
    /// cannot outlive the process, so it needs a journal that survives the writer.
    /// This rig's action increments a counter in its own address space, so
    /// declaring `External` would be a false claim about the work being measured —
    /// and it would put a durable append inside every timed window, which is a
    /// different measurement from the one this rig publishes (see the journal
    /// section of this directory's README).
    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u64)) -> Result<(), EcsError> {
        let (auth, _value) = call;
        auth.check(self.required_caps())?;
        self.0.set(self.0.get().saturating_add(1));
        Ok(())
    }

    fn domain_id(&self) -> &'static str {
        "bench::tally"
    }
}

/// The effect scope every bot in this rig dispatches under.
///
/// A fixed identity rather than a minted one, so two runs of the same scenario
/// derive the same action identities and read back the same facts: the rig
/// measures a repeated workload, and an identity that moved under it would make
/// two runs incomparable. Nothing here dispatches anything durable — the action
/// is [`EffectLifetime::Local`] — so what this scope buys is the effect path
/// itself running: the ledger write, the warrant, and the broker's revalidation.
fn effects() -> Result<EffectScope, BenchError> {
    let environment = EnvironmentId::from_hex(&lgwks_std::hex::encode([0x21_u8; 16]))?;
    let mut broker = Broker::new();
    broker.register(environment)?;
    // A fixed 32-byte revision digest, not a computed one: the flow revision is
    // folded into every action identity, and a revision that moved between runs
    // would make two runs of one workload incomparable.
    let flow = FlowRevision::from_tagged("blake3_256", &lgwks_std::hex::encode([0x5a_u8; 32]))?;
    let run = RunId::from_hex(&lgwks_std::hex::encode([0x01_u8; 16]))?;
    let identity = EffectIdentity::new(run, environment, flow);
    Ok(EffectScope::new(
        identity,
        broker,
        Box::new(MemoryJournal::new()),
    ))
}

/// The condition: fire on an even observed value.
///
/// Non-constant on purpose: a condition that always returned `true` would make
/// "fired" identical to "changed" and would not exercise the `Evaluate` verb.
///
/// It also counts its own evaluations. That counter is the fairness gate's
/// second axis, and it is what catches a baseline that reaches the right answer
/// by a cheaper route.
struct Even(Rc<Cell<u64>>);

impl Evaluate<u64> for Even {
    fn check(&self, value: &u64) -> Result<bool, EcsError> {
        self.0.set(self.0.get().saturating_add(1));
        Ok(value.is_multiple_of(2))
    }

    fn condition_id(&self) -> &'static str {
        "bench::even"
    }
}

// ── The baseline ─────────────────────────────────────────────────────────────

/// One chain of the hand-rolled equivalent: poll, detect change, evaluate, act.
///
/// This is the null hypothesis. It holds no ECS world, no ledger, no `Auth`
/// proof, and no capability set -- it is what the work costs when nothing is
/// being guaranteed. Whatever the bot costs above this is the price of the
/// guarantees, and that price is the interesting number.
pub(super) struct HandRolled {
    period: u64,
    entries: usize,
    prior: Option<u64>,
}

impl HandRolled {
    fn new(period: u64, entries: usize) -> Self {
        Self {
            period,
            entries,
            prior: None,
        }
    }

    /// One tick, mirroring the bot's semantics exactly: walk the entries only
    /// when the polled value moved, and evaluate the condition once per entry.
    ///
    /// The loop over `entries` is load-bearing, not decoration. Returning
    /// `self.entries` as a count is arithmetically identical and does none of
    /// the work; `evals` counts the evaluations so a baseline that cuts that
    /// corner cannot pass the fairness gate unnoticed.
    fn tick(&mut self, tick: u64, evals: &Cell<u64>) -> u64 {
        let period = if self.period == 0 { 1 } else { self.period };
        let value = tick / period;
        if self.prior == Some(value) {
            return 0;
        }
        self.prior = Some(value);
        let mut fired = 0;
        for _ in 0..self.entries {
            evals.set(evals.get().saturating_add(1));
            if value.is_multiple_of(2) {
                fired += 1;
            }
        }
        fired
    }
}

// ── Building both engines ────────────────────────────────────────────────────

pub(super) fn build_bot(
    scenario: &Scenario,
    clock: &Clock,
    tally: &Rc<Cell<u64>>,
    evals: &Rc<Cell<u64>>,
) -> Result<Bot, BenchError> {
    let mut builder = Bot::builder("bench").observe(Source {
        clock: clock.clone(),
        period: scenario.period,
    });
    for _ in 0..scenario.entries {
        builder = builder.on(Even(evals.clone()), Tally(tally.clone()));
    }
    for _ in 1..scenario.sources {
        builder = builder.observe(Source {
            clock: clock.clone(),
            period: scenario.period,
        });
        for _ in 0..scenario.entries {
            builder = builder.on(Even(evals.clone()), Tally(tally.clone()));
        }
    }
    Ok(builder.with_effects(effects()?).build(&GrantSet::empty())?)
}

pub(super) fn build_handrolled(scenario: &Scenario) -> Vec<HandRolled> {
    (0..scenario.sources)
        .map(|_| HandRolled::new(scenario.period, scenario.entries))
        .collect()
}

/// Advance the bot `ticks` ticks from `tick`, returning the effects it fired.
///
/// One tick loop for the timing rows, the allocation window and the profile,
/// because three copies of `clock.set` + `bot.tick` + advance is three places a
/// round's workload could drift apart — and a per-tick cost measured over a
/// different workload than the one reported is the defect this rig exists to
/// refuse.
pub(super) fn run_bot_ticks(
    bot: &mut Bot,
    clock: &Clock,
    tick: &mut u64,
    ticks: u64,
) -> Result<u64, BenchError> {
    let mut fired = 0u64;
    for _ in 0..ticks {
        clock.set(*tick);
        fired = fired.saturating_add(bot.tick()? as u64);
        *tick = tick.saturating_add(1);
    }
    Ok(fired)
}

/// Advance the hand-rolled chains `ticks` ticks from `tick`, returning their fires.
pub(super) fn run_base_ticks(
    chains: &mut [HandRolled],
    evals: &Cell<u64>,
    tick: &mut u64,
    ticks: u64,
) -> u64 {
    let mut fired = 0u64;
    for _ in 0..ticks {
        fired = fired.saturating_add(chains.iter_mut().map(|c| c.tick(*tick, evals)).sum::<u64>());
        *tick = tick.saturating_add(1);
    }
    fired
}
