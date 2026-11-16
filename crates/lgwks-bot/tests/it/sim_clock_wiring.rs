//! Deterministic simulation of the declared clock **on the real run path**.
//!
//! `sim_clock.rs` proves the clock is a correct counter. It does not prove
//! anything is *built on* it — a clock nothing reads is a type, not a feature,
//! and an unwired clock is the defect this whole family exists to prevent. Every
//! test here drives work that production also runs: a [`Supervisor`] budget, a
//! [`Supervisor::spawn_repeating`] loop, a [`Host`] run, and a
//! [`within`](lgwks_bot::script::within) step. The oracle is a seeded trace
//! hash plus the terminal outcome each run reported.
//!
//! The central claim, and the reason this family is not a duplicate of
//! `sim_clock.rs`: **a caller-advanceable clock governs those deadlines**, so a
//! logical advance produces the *same* refusal a real overrun produces, at
//! zero real cost. A 24-hour budget is exhausted by one arithmetic step here;
//! the wall-clock twin of each test spends real seconds and agrees on every
//! field but the elapsed time.
//!
//! What the family proves, and what it deliberately does not:
//!
//! - **Proved**: the declared clock reaches the real budget checks; a virtual
//!   advance refuses with the real error variant and no real wait; the same
//!   program on a wall clock reaches the same verdict by real elapsed time;
//!   two tenants on two clocks cannot see each other's time; the watchdog stays
//!   independent while all of this happens.
//! - **Not claimed**: that a real `sleep` resolves at a virtual instant. The
//!   logical clock governs deadlines; the engine's timer governs when a future
//!   wakes. No test here asserts otherwise.

#![cfg(all(feature = "rt", feature = "time", feature = "sync", feature = "script"))]

use crate::sim::seed;

use std::time::Duration;

use lgwks_bot::rt::clock::{Clock, ClockError, TimeSource};
use lgwks_bot::rt::supervise::{Budget, Outcome, Supervisor, TaskOutcome};
use lgwks_bot::rt::task::JoinSet;
use lgwks_bot::script::{Scope, Tenant};
use lgwks_bot::task::{Disposition, Report};
use std::num::NonZeroU64;
use std::sync::Arc;

use seed::{Rng, Trace};

/// The band of seeds this family sweeps.
///
/// Disjoint from `sim_clock.rs`'s band would be nicer and is not done: both
/// families draw from the shared `0..64` space so a failure reported against
/// either names the same seeds, which is worth more than non-overlap between
/// two families that sweep the same generator.
const BAND_FIRST: u64 = 0;
const BAND_COUNT: u64 = 64;

/// The seeds this family sweeps.
fn seeds() -> impl Iterator<Item = u64> {
    (0..BAND_COUNT).map(|index| BAND_FIRST.saturating_add(index))
}

/// How long a loop may take to end on its budget before the run is drained.
///
/// A ceiling on a failure, never a wait the passing case spends: a loop whose
/// budget is exhausted ends at its next clock poll and `wait_idle` returns then.
/// Only a loop that never reads its budget reaches this, and the drain then
/// reports it as not succeeded, which is the defect the family catches.
const STOP_CEILING: Duration = Duration::from_secs(30);

/// Iterations the control scenario's loop runs before its budget stops it.
///
/// Small and fixed: the control is about *which* bound ended the run, and a
/// large count would make that question take as long as the count. One
/// non-zero value cannot express the run at all — [`Budget::Iterations`]
/// rejects zero by design — so the constant is the smallest honest one.
const FROZEN_ITERATIONS: u64 = 4;

/// How long a real `sleep` in these bodies waits, per seeded draw.
///
/// Small enough that the whole family's wall-clock twin is affordable, large
/// enough that a logical advance is unambiguously the thing that stopped the
/// run: the distinction between "the clock moved 24 hours" and "the clock moved
/// 200 microseconds" is the entire claim.
const REAL_WAIT_CEILING: Duration = Duration::from_millis(5);

/// The seeded shape of one scenario, so the two twin tests drive the *same*
/// program and can only differ in which clock governed it.
#[derive(Clone, Copy, Debug)]
struct Plan {
    /// The budget each run is given, from a small set so several tiers are hit.
    budget: Duration,
    /// Whether the scenario advances the clock at all before observing the
    /// outcome. A false is the control case: nothing should expire on its own.
    advances: bool,
    /// The multiplier applied to the budget to decide whether it is exhausted.
    over: u64,
    /// How many work units the body performs, so a truncated run is visible.
    work_units: u64,
}

/// How a scenario's budget comes to be spent.
///
/// The two timelines have to be exercised differently, and conflating them is
/// the mistake this sum type exists to prevent: a wall clock cannot be advanced
/// at all, so a wall twin asked to "advance" would be testing
/// [`ClockError::NotVirtual`] rather than a budget. A wall run spends its
/// budget by *waiting*; a virtual run spends it by *arithmetic*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpendsHow {
    /// An advance lands on the clock while the loop runs.
    Advanced,
    /// Nothing moves the clock, and the loop runs on an iteration budget so it
    /// terminates.
    Unmoved,
    /// Real time elapses until the budget is spent. Wall clocks only.
    Waited,
}

/// The plan for one seed.
///
/// Deterministic in the seed and independently chosen rather than tuned to
/// produce a particular outcome: the draw is the same generator the rest of the
/// simulation substrate uses, so a recorded hash means the same thing here.
fn plan(seed: u64) -> Plan {
    let mut rng = Rng::new(seed ^ 0xd1ea_d1ea_0bad_f00d);
    let budget_millis = u64::from(rng.between(1, 4_000));
    Plan {
        budget: Duration::from_millis(budget_millis),
        // One draw in three spends nothing at all, which is the case that
        // catches an implementation that expires a step on construction rather
        // than on the bound being met.
        advances: rng.chance(300),
        over: u64::from(rng.between(1, 50)),
        work_units: u64::from(rng.between(0, 4)),
    }
}

/// The terminal fact one scenario observed, as a trace.
fn observe(trace: &mut Trace, label: &str, value: &str) {
    trace.record(label);
    trace.record(value);
}

/// Drive one future to completion on the crate's own synchronous entry.
///
/// [`lgwks_bot::script::block_on`] rather than a runtime built here, so the family runs
/// on exactly the entry point a consumer would use and a change to that entry
/// point is caught by these tests too.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    lgwks_bot::script::block_on(future)
}

/// A defect this family found.
///
/// Returned as `Err` rather than reached through `panic!`/`expect!`: the
/// workspace forbids both, and a failure that arrives as a value prints its
/// message exactly like an assertion's — with the seed that produced it, which
/// is the whole point of the simulation substrate.
#[derive(Debug)]
struct Defect(String);

impl std::fmt::Display for Defect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Defect {}

/// Record the advance's verdict in the slot the advancing task owns.
///
/// Poisoning recovers rather than propagates: the slot holds a `String` set once
/// and read once, and a panic in the scenario would already have failed the
/// test, so propagating here would replace a real message with a second one.
fn record_verdict(slot: &Arc<std::sync::Mutex<String>>, verdict: &str) {
    if let Ok(mut held) = slot.lock() {
        held.clear();
        held.push_str(verdict);
    }
}

/// Run `plan`'s wall-clock twin: the same body and budget shape on a wall clock.
///
/// The counterpart to a virtual run, and the half of the comparison that makes a
/// virtual run meaningful. Two differences from the virtual twin, both forced by
/// what a wall clock is:
///
/// - The budget is capped at [`REAL_WAIT_CEILING`], so "real time passed" costs
///   the family milliseconds rather than the drawn seconds. The *decision* is
///   identical; only the wait is shortened.
/// - A wall clock cannot be advanced — that is [`ClockError::NotVirtual`] and
///   it is the contract — so an advancing scenario **waits** its budget out
///   instead. Same decision, different mechanism, which is exactly what the
///   comparison claims.
///
/// The clock is returned as well as the run, so a caller can inspect the
/// watchdog the run left behind.
fn wall_twin(plan: Plan) -> Result<(Observed, Clock), Box<dyn std::error::Error>> {
    let short = Plan {
        budget: plan.budget.min(REAL_WAIT_CEILING),
        over: plan.over,
        advances: plan.advances,
        work_units: plan.work_units,
    };
    let how = if plan.advances {
        SpendsHow::Waited
    } else {
        SpendsHow::Unmoved
    };
    let clock = Clock::wall();
    let run = block_on(run_repeating(clock.clone(), short, how))?;
    Ok((run, clock))
}

/// The refusal a step must have reported, or a [`Defect`] naming what it did.
///
/// One place so every "expected a refusal, got something else" assertion in this
/// family reads the same and names the same two facts: which step, and what came
/// back instead.
fn refusal(
    result: Result<(), lgwks_bot::script::FlowError>,
    step: &str,
) -> Result<lgwks_bot::script::FlowError, Defect> {
    match result {
        Err(error) => Ok(error),
        Ok(()) => Err(Defect(format!(
            "step {step:?} returned success; its budget was already spent"
        ))),
    }
}

/// What one run of the repeating scenario observed.
///
/// A struct rather than a bare bool so the twin assertion can compare the
/// terminal state and the trace that recorded it together, and so a scenario
/// that reported *nothing* is distinguishable from one that reported the wrong
/// thing — which are different defects and must not share a branch.
#[derive(Debug)]
struct Observed {
    /// The trace the run produced, which is its replay receipt.
    ///
    /// Held rather than discarded so a scenario that recorded nothing is
    /// distinguishable from one that recorded the wrong thing. The hash is what
    /// two runs are compared by; `Trace` itself has no `PartialEq`, by design —
    /// a receipt is a digest, not a value.
    trace: Trace,
    /// The run's own success predicate.
    ///
    /// `Budget::For` reports exhaustion as a *succeeded* task: the loop ended on
    /// its budget rather than being cancelled, and this is the only public
    /// predicate that says so. `None` means no terminal outcome was reported at
    /// all, which is its own failure.
    succeeded: Option<bool>,
}

/// Run a repeating task under `clock` and report what the budget decided.
///
/// This is the production path: `Supervisor::spawn_repeating` places a
/// `repeat_on` loop behind a permit, exactly as a long-running bot loop is
/// started, and the loop's own budget check reads the supervisor's clock.
///
/// The advance is delivered **while the loop is running**, not before the drain.
/// That ordering is the whole determinism argument: `shutdown` cancels first,
/// and a task that has not yet been polled observes that cancel before it ever
/// reads its budget, so an advance applied before the drain would race the
/// cancel and the test would be asserting on which of the two the executor
/// happened to reach first. Parked in a body and advanced from a second task,
/// the loop's *budget* is what stops it, on every run.
async fn run_repeating(
    clock: Clock,
    plan: Plan,
    how: SpendsHow,
) -> Result<Observed, Box<dyn std::error::Error>> {
    use lgwks_bot::rt::sync::Notify;

    let mut trace = Trace::new();
    // The budget the loop runs under.
    //
    // A scenario that advances uses `Budget::For`, because the advance is what
    // must stop it. A scenario that does *not* advance cannot use `Budget::For`:
    // a clock nobody moved cannot exhaust a duration, so a `For` loop with an
    // always-returning body would spin forever — which is the correct semantics
    // and an unusable test. That scenario gets an iteration budget instead, so
    // it terminates and what is compared is the *terminal state*, not how long
    // each side took to reach it.
    let budget = if how == SpendsHow::Unmoved {
        Budget::Iterations(
            NonZeroU64::new(FROZEN_ITERATIONS).ok_or("a frozen budget of zero iterations")?,
        )
    } else {
        Budget::For(plan.budget)
    };
    let mut supervisor = Supervisor::with_clock(1, clock.clone());
    let parked = Arc::new(Notify::new());
    let signal = Arc::clone(&parked);
    let parks_body = how != SpendsHow::Unmoved;
    supervisor
        .spawn_repeating(budget, move |_tick| {
            let signal = Arc::clone(&signal);
            async move {
                signal.notify_one();
                // Park until the budget is spent. A body that returned
                // immediately would let the loop reach its bound only by
                // spinning, and the stop would then race an unbounded number
                // of iterations instead of being observed at one.
                if parks_body {
                    std::future::pending::<()>().await;
                }
            }
        })
        .await;

    // Whether this scenario's clock will ever move. A scenario that does not
    // advance must not park its body either: a parked body on a frozen clock is
    // stopped only by the drain, and parking it would make the control case
    // measure the drain rather than the budget — which is exactly the
    // distinction the control exists to draw.
    let advance_clock = clock.clone();
    // The advance's verdict travels out as a plain string rather than as a
    // trace mutation: the future must own what it writes, and a `Trace` moved
    // into a future cannot be written to from outside it. The fact is recorded
    // into the trace below once the future has settled.
    let advance_verdict = Arc::new(std::sync::Mutex::new(String::from("pending")));
    // Two handles to one slot: the advancing task owns one and the reader owns
    // the other. Cloning before the `async move` is what keeps the reader's
    // handle from being captured into the future it is trying to read after.
    let writing_verdict = Arc::clone(&advance_verdict);
    let advance = async move {
        parked.notified().await;
        if how == SpendsHow::Advanced {
            let over = match u32::try_from(plan.over) {
                Ok(over) => over,
                // An over-advance this host cannot express is recorded as its own
                // verdict rather than folded into the largest one, because the
                // arms below are compared against each other.
                Err(_too_wide) => {
                    record_verdict(&writing_verdict, "over-advance-unrepresentable");
                    return;
                }
            };
            let target = plan.budget.saturating_mul(over);
            match advance_clock.advance(target) {
                Ok(_) => record_verdict(&writing_verdict, "landed"),
                Err(ClockError::NotVirtual) => {
                    record_verdict(&writing_verdict, "refused-not-virtual");
                }
                Err(ClockError::OutOfRange { .. }) => {
                    record_verdict(&writing_verdict, "refused-range");
                }
                // `ClockError` is `#[non_exhaustive]`, so a new variant is a new
                // fact rather than a compile break. Recording it and carrying on
                // is honest: the run's terminal state is asserted below whatever
                // the advance did.
                Err(_) => record_verdict(&writing_verdict, "refused-other"),
            }
        } else {
            record_verdict(&writing_verdict, "skipped");
        }
    };

    // Deliver the advance (or, for the wall twin, nothing: its clock moves by
    // itself), then wait for the loop to end **on its own** before the drain.
    // `shutdown` cancels first, so a drain that arrives before the loop has read
    // its exhausted budget reports a cancel, and which of the two the executor
    // reached first has nothing to do with the clock under test. A fixed settle
    // after the advance was that race: under a CI load of 90 the advance landed
    // after the settle had already elapsed (seed 51), and the drain beat the
    // loop's one-millisecond clock poll. `wait_idle` is the loop's own end, so the
    // budget is what ends the run on every host; [`STOP_CEILING`] bounds only the
    // failure in which the loop never reads its budget, which then reaches the
    // drain and is reported as not succeeded.
    match how {
        SpendsHow::Advanced => advance.await,
        SpendsHow::Waited => record_verdict(&advance_verdict, "waited"),
        SpendsHow::Unmoved => {}
    }
    let ended = match lgwks_bot::rt::time::timeout(STOP_CEILING, supervisor.wait_idle()).await {
        Ok(_joined) => "on-its-own",
        Err(_elapsed) => "at-the-ceiling",
    };
    observe(&mut trace, "loop-ended", ended);
    let report = supervisor.shutdown().await;

    // What the clock did is recorded before what the run decided, so the trace
    // reads in the order the two facts happened.
    let verdict = match advance_verdict.lock() {
        Ok(held) => held.clone(),
        Err(_poisoned) => String::from("poisoned"),
    };
    observe(&mut trace, "advance", &verdict);
    // `is_success` rather than a rendered state string: the predicate is the
    // public contract, and a test matching on rendered text would break the
    // moment a display changed without the contract changing.
    let succeeded = report.outcomes().first().map(TaskOutcome::is_success);
    observe(
        &mut trace,
        "repeating-succeeded",
        match succeeded {
            Some(true) => "yes",
            Some(false) => "no",
            None => "absent",
        },
    );
    // The wall watchdog is a separate fact from the logical counter: it must
    // not have moved by however much logical time the advance spent.
    trace.record_number(
        "watchdog-under-a-second",
        u64::from(clock.wall_watchdog().elapsed() < Duration::from_secs(1)),
    );
    Ok(Observed { trace, succeeded })
}

/// The seed is the receipt: the same seed runs the same plan on the real path.
#[test]
fn the_same_seed_replays_the_same_budget_plan() -> Result<(), Box<dyn std::error::Error>> {
    for seed in seeds() {
        let first = format!("{:?}", plan(seed));
        let second = format!("{:?}", plan(seed));
        assert_eq!(
            first, second,
            "seed {seed} must produce one plan; a differing plan means the \
             scenario is not a replay receipt"
        );
    }
    Ok(())
}

/// The plans really do cover both sides of the boundary.
///
/// A family whose seeds never land on an exhausted budget proves nothing about
/// expiry, and a family whose seeds always do proves nothing about honouring a
/// budget. Both halves are asserted, so a change to the generator that skewed
/// the band is a failure rather than a silently weaker test.
#[test]
fn the_band_covers_exhausted_and_unexhausted_budgets_alike() {
    let mut advancing: u32 = 0;
    let mut passing: u32 = 0;
    for seed in seeds() {
        let plan = plan(seed);
        if plan.advances && plan.over >= 2 {
            advancing = advancing.saturating_add(1);
        }
        if !plan.advances || plan.over < 2 {
            passing = passing.saturating_add(1);
        }
    }
    assert!(advancing > 0, "no seed exhausts its budget");
    assert!(passing > 0, "no seed leaves its budget unspent");
}

mod loops;
mod scale;
mod tenants;
mod within;
