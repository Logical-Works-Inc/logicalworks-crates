//! Simulations of the worker's world: the step log a judge reads and the plan file
//! the driver writes.
//!
//! The upstream calls wait on the clock the world is given, the wall clock in the
//! binary, so a virtual clock proves the wait here with no real time passing:
//! which step bodies are logged, how often, what a hostile plan file is refused
//! with, and that a call is pending until the clock reaches its latency.

use std::future::Future;
use std::path::PathBuf;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use lgwks_bot::clock::Clock;
use lgwks_std::seeded::Seeded;

use crate::world::{Call, World, WorldError};

/// The clock of a world that never waits real time.
fn clock() -> Clock {
    Clock::virtual_at(Duration::ZERO)
}

/// A scratch directory of this test run, removed when it goes.
struct Scratch(PathBuf);

impl Scratch {
    /// A fresh directory named for `name` and this process.
    fn new(name: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("lgwks-restate-sim-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::warn!(%error, "a scratch directory was not removed");
        }
    }
}

/// The ten steps' logged names, as the driver's judge lists them.
const LOGGED: [&str; 9] = [
    "s1-intake",
    "s2-validate",
    "s3-outside",
    "s4-merge",
    "s5-prompt",
    "s6-model",
    "s7-check",
    "s9-apply",
    "s10-finish",
];

/// Seeded runs execute seeded steps, some of them twice, as a crashed worker would
/// run a step body again: the log holds one line per execution, in the judge's
/// three-word format, and a count of them equals the model's.
#[test]
fn sim_local_steps_log_one_line_per_execution() -> Result<(), String> {
    for seed in 1..=25_u64 {
        let scratch = Scratch::new(&format!("steps-{seed}"))?;
        let plan = scratch.0.join("plan.txt");
        std::fs::write(&plan, "0 0\n".repeat(8)).map_err(|e| e.to_string())?;
        let (steps, attempts) = (scratch.0.join("exec.log"), scratch.0.join("attempts.log"));
        let world = World::load(&plan, Some(&steps), &attempts, clock()).map_err(|e| e.to_string())?;
        let mut stream = Seeded::from_seed(seed);
        let mut model = std::collections::BTreeMap::new();
        for _ in 0..200 {
            let ordinal = u32::try_from(stream.below(8).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let at = usize::try_from(stream.below(5).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let step = [LOGGED[0], LOGGED[1], LOGGED[3], LOGGED[4], LOGGED[6]][at];
            let tenant = if ordinal % 2 == 0 { "alpha" } else { "beta" };
            world.ran(tenant, ordinal, step).map_err(|e| e.to_string())?;
            *model.entry(format!("{tenant} {ordinal} {step}")).or_insert(0_u32) += 1;
        }
        let log = std::fs::read_to_string(&steps).map_err(|e| e.to_string())?;
        let mut seen = std::collections::BTreeMap::new();
        for line in log.lines() {
            assert_eq!(line.split(' ').count(), 3, "{line}");
            *seen.entry(line.to_owned()).or_insert(0_u32) += 1;
        }
        assert_eq!(seen, model, "seed {seed}");
        assert!(!attempts.exists() || std::fs::read_to_string(&attempts).map_err(|e| e.to_string())?.is_empty());
    }
    Ok(())
}

/// Without a step log nothing is written for steps, and the attempt log stays
/// empty for a run that never retried.
#[test]
fn a_world_without_a_step_log_writes_none() -> Result<(), String> {
    let scratch = Scratch::new("nosteps")?;
    let plan = scratch.0.join("plan.txt");
    std::fs::write(&plan, "0 0\n").map_err(|e| e.to_string())?;
    let attempts = scratch.0.join("attempts.log");
    let world = World::load(&plan, None, &attempts, clock()).map_err(|e| e.to_string())?;
    world.ran("alpha", 0, LOGGED[0]).map_err(|e| e.to_string())?;
    assert!(!scratch.0.join("exec.log").exists());
    Ok(())
}

/// An upstream call is parked on the world's own clock: pending until the clock
/// reaches its latency, still pending one nanosecond short, and answered with the
/// reference value on the advance onto it, for both upstreams. No real time
/// passes, so the same code that waits 2 ms and 20 ms live waits nothing here.
#[test]
fn a_call_waits_on_the_world_clock_and_not_on_real_time() -> Result<(), String> {
    let scratch = Scratch::new("clock")?;
    let plan = scratch.0.join("plan.txt");
    std::fs::write(&plan, "0 0\n").map_err(|e| e.to_string())?;
    for (call, latency, step, want) in [
        (Call::Outside, Duration::from_millis(2), "s3-outside", 1_u64),
        (Call::Model, Duration::from_millis(20), "s6-model", 2_u64),
    ] {
        let clock = clock();
        let attempts = scratch.0.join(format!("attempts-{step}.log"));
        let world = World::load(&plan, None, &attempts, clock.clone()).map_err(|e| e.to_string())?;
        let mut waiting = std::pin::pin!(world.call(call, "alpha", 0, step));
        let mut context = Context::from_waker(Waker::noop());
        assert!(waiting.as_mut().poll(&mut context).is_pending(), "{step}");
        assert!(clock.has_waiters(), "{step} is not parked on the clock");
        clock
            .advance(latency.saturating_sub(Duration::from_nanos(1)))
            .map_err(|e| e.to_string())?;
        assert!(waiting.as_mut().poll(&mut context).is_pending(), "{step}: one ns short answered");
        clock.advance(Duration::from_nanos(1)).map_err(|e| e.to_string())?;
        match waiting.as_mut().poll(&mut context) {
            Poll::Ready(Ok(value)) => assert_eq!(value, want, "{step}"),
            other => return Err(format!("{step}: the advance onto the latency gave {other:?}")),
        }
    }
    Ok(())
}

/// A hostile plan file is refused whole and typed: a line of three words, a
/// negative number, a missing file.
#[test]
fn sim_hostile_plan_files_are_refused_typed() -> Result<(), String> {
    let scratch = Scratch::new("plans")?;
    let attempts = scratch.0.join("attempts.log");
    for (at, bad) in ["1 2 3\n", "-1 2\n", "x y\n", "1\n", "0 0\n1  1\n"].iter().enumerate() {
        let plan = scratch.0.join(format!("plan-{at}.txt"));
        std::fs::write(&plan, bad).map_err(|e| e.to_string())?;
        let refused = World::load(&plan, None, &attempts, clock());
        assert!(matches!(refused, Err(WorldError::PlanMalformed)), "{bad:?}");
    }
    let missing = World::load(&scratch.0.join("absent.txt"), None, &attempts, clock());
    assert!(matches!(missing, Err(WorldError::Io { .. })));
    Ok(())
}
