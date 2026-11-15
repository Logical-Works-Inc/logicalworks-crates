//! What the flow talks to: two in-process upstreams with a declared, fixed
//! latency, a seeded failure plan, the retry counters and the log that records
//! which step bodies ran.
//!
//! The upstreams live in this process, so the numbers measure the engine (admission,
//! group-committed records, retry, the approval round trip) and not a network.
//! Every failure is decided before the first run starts, from one seeded stream
//! walked in run order, so a seed names one plan whatever the scheduler does.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lgwks_bot::clock::Clock;
use lgwks_bot::error::KernelError;
use lgwks_bot::script::FlowError;
use lgwks_bot::{DispatchCertainty, EcsError};
use lgwks_std::seeded::Seeded;

use crate::error::BenchError;

/// Attempts an upstream call gets under the crate's retry policy.
pub(crate) const ATTEMPTS: NonZeroU32 = match NonZeroU32::new(5) {
    Some(attempts) => attempts,
    None => NonZeroU32::MIN,
};

/// The share of attempts the workload makes fail, in whole percent.
pub(crate) const FAILURE_PERCENT: u64 = 20;

/// The steps whose body runs and so writes the step log: every `remember` step.
/// The approval (step 8) is the crate's own record and writes none.
pub(crate) const LOGGED_STEPS: [&str; 9] = [
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

/// What each pure step adds to the running total: its position in the flow.
pub(crate) const STEP_WEIGHTS: [u64; 7] = [1, 2, 4, 5, 7, 9, 10];

/// The bytes an approval's payload holds: one run number, little-endian.
pub(crate) const PAYLOAD_BYTES: u64 = 8;

/// Which upstream a call goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Call {
    /// The outside call of step 3.
    Outside,
    /// The model call of step 6.
    Model,
}

impl Call {
    /// The retry block's step name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Outside => "outside-call",
            Self::Model => "model-call",
        }
    }

    /// What the upstream returns for `ordinal`: a value the reference model knows.
    const fn value(self, ordinal: u32) -> u64 {
        let ordinal = ordinal as u64;
        match self {
            Self::Outside => ordinal.wrapping_mul(3).wrapping_add(1),
            Self::Model => ordinal.wrapping_mul(5).wrapping_add(2),
        }
    }
}

/// The fixed latencies of the upstreams and of the human decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Latencies {
    /// The outside call.
    pub(crate) outside: Duration,
    /// The model call.
    pub(crate) model: Duration,
    /// How long before the approval is auto-granted.
    pub(crate) approval: Duration,
}

impl Latencies {
    /// The workload the issue names: 2 ms, 20 ms and 5 ms.
    pub(crate) const WORKLOAD: Self = Self {
        outside: Duration::from_millis(2),
        model: Duration::from_millis(20),
        approval: Duration::from_millis(5),
    };

    /// No waiting at all: for simulations that want the engine's logic alone.
    #[cfg(test)]
    pub(crate) const ZERO: Self = Self {
        outside: Duration::ZERO,
        model: Duration::ZERO,
        approval: Duration::ZERO,
    };
}

/// The reference model: what a run over `ordinal` must return, computed
/// without any of the engine.
pub(crate) fn expected(ordinal: u32) -> u64 {
    let pure: u64 = STEP_WEIGHTS.iter().sum();
    u64::from(ordinal)
        .wrapping_add(pure)
        .wrapping_add(Call::Outside.value(ordinal))
        .wrapping_add(Call::Model.value(ordinal))
        .wrapping_add(PAYLOAD_BYTES)
}

/// How many attempts of each call fail for one run before one succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Faults {
    /// Failing attempts of the outside call.
    pub(crate) outside: u32,
    /// Failing attempts of the model call.
    pub(crate) model: u32,
}

/// Draw how many leading attempts fail: each attempt fails with
/// `percent / 100`, up to the attempts the retry gets.
fn draw_failures(stream: &mut Seeded, percent: u64) -> Result<u32, BenchError> {
    let mut failures = 0;
    while failures < ATTEMPTS.get() {
        let roll = stream
            .below(100)
            .map_err(|source| BenchError::Draw { source })?;
        if roll >= percent {
            break;
        }
        failures += 1;
    }
    Ok(failures)
}

/// An append-only text file many threads write whole lines to.
#[derive(Debug)]
pub(crate) struct AppendLog {
    /// The file, behind a lock so two lines never interleave.
    file: Mutex<File>,
}

impl AppendLog {
    /// Open `path` for appending, creating it.
    ///
    /// # Errors
    ///
    /// [`BenchError::Io`] when the file cannot be opened.
    pub(crate) fn open(path: &Path, context: &'static str) -> Result<Self, BenchError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| BenchError::Io { context, source })?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    /// Append one line, newline included, in a single write.
    ///
    /// # Errors
    ///
    /// The write's error, or a poisoned lock as one.
    pub(crate) fn append(&self, line: &str) -> io::Result<()> {
        let mut file = self.file.lock().map_err(|poisoned| {
            lgwks_std::trace::debug!(error = %poisoned, "the log's lock was poisoned");
            io::Error::other("the log's lock was poisoned")
        })?;
        file.write_all(format!("{line}\n").as_bytes())
    }
}

/// Everything the flow reaches that is not the engine.
#[derive(Debug)]
pub(crate) struct World {
    /// One entry per run, by ordinal.
    faults: Vec<Faults>,
    /// The fixed latencies.
    latencies: Latencies,
    /// The clock every upstream wait and every approval delay is made on: the
    /// wall clock live, a virtual one a simulation moves itself.
    clock: Clock,
    /// Attempts beyond the first that ran: retries taken.
    retries: AtomicU64,
    /// Failures the upstreams injected.
    injected: AtomicU64,
    /// Which step bodies ran, when recording them was asked for.
    steps: Option<AppendLog>,
}

impl World {
    /// The world for `runs` runs under `seed`, failing `percent` of attempts,
    /// waiting its latencies on `clock`.
    ///
    /// The plan is drawn here, in run order, outside call first: the stream is
    /// walked by this one loop and nothing else draws from it.
    ///
    /// # Errors
    ///
    /// [`BenchError::Draw`] if the stream refuses a draw.
    pub(crate) fn plan(
        seed: u64,
        runs: u32,
        percent: u64,
        latencies: Latencies,
        clock: Clock,
        steps: Option<AppendLog>,
    ) -> Result<Self, BenchError> {
        let mut stream = Seeded::from_seed(seed);
        let mut faults = Vec::new();
        for _ in 0..runs {
            let outside = draw_failures(&mut stream, percent)?;
            let model = draw_failures(&mut stream, percent)?;
            faults.push(Faults { outside, model });
        }
        Ok(Self {
            faults,
            latencies,
            clock,
            retries: AtomicU64::new(0),
            injected: AtomicU64::new(0),
            steps,
        })
    }

    /// The failures planned for `ordinal`, or `None` for a run outside the plan.
    pub(crate) fn planned(&self, ordinal: u32) -> Option<Faults> {
        let at = usize::try_from(ordinal).ok()?;
        self.faults.get(at).copied()
    }

    /// The plan as a whole.
    #[cfg(test)]
    pub(crate) fn plans(&self) -> &[Faults] {
        &self.faults
    }

    /// The fixed latencies.
    pub(crate) const fn latencies(&self) -> Latencies {
        self.latencies
    }

    /// The clock the world's waits are made on.
    pub(crate) const fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Retries taken so far.
    pub(crate) fn retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    /// Failures injected so far.
    pub(crate) fn injected(&self) -> u64 {
        self.injected.load(Ordering::Relaxed)
    }

    /// One attempt of an upstream call: wait its latency, then either fail as the
    /// plan says or return the value the reference model knows.
    ///
    /// A failure is a [`DispatchCertainty::NotDelivered`] domain error, which the
    /// crate classes `RetryClass::Safe`: nothing reached the upstream, so a retry
    /// is a retry.
    pub(crate) async fn call(
        &self,
        call: Call,
        ordinal: u32,
        attempt: u32,
    ) -> Result<u64, FlowError> {
        let delay = match call {
            Call::Outside => self.latencies.outside,
            Call::Model => self.latencies.model,
        };
        if !delay.is_zero() {
            self.clock.sleep(delay).await;
        }
        if attempt > 1 {
            self.retries.fetch_add(1, Ordering::Relaxed);
        }
        let planned = self.planned(ordinal).ok_or_else(|| {
            FlowError::failed(format!("run {ordinal} is outside the failure plan"))
        })?;
        let failing = match call {
            Call::Outside => planned.outside,
            Call::Model => planned.model,
        };
        if attempt <= failing {
            self.injected.fetch_add(1, Ordering::Relaxed);
            lgwks_std::trace::debug!(
                call = call.label(),
                ordinal,
                attempt,
                "an upstream attempt fails as planned"
            );
            return Err(FlowError::Bot {
                at: std::sync::Arc::from(call.label()),
                // lgwks_bot 9: the kernel owns DomainError, and each layer wraps it by From (#990).
                source: Box::new(EcsError::from(KernelError::DomainError {
                    domain: format!("upstream::{}", call.label()),
                    certainty: DispatchCertainty::NotDelivered,
                    cause: "injected transient failure".to_owned(),
                })),
            });
        }
        Ok(call.value(ordinal))
    }

    /// Record that a step body is about to run, when recording is on.
    ///
    /// # Errors
    ///
    /// [`FlowError`] when the line cannot be written: a flow that cannot say
    /// what it ran must not run it.
    pub(crate) fn ran(&self, tenant: &str, ordinal: u32, step: &str) -> Result<(), FlowError> {
        match self.steps.as_ref() {
            Some(log) => log
                .append(&format!("{tenant} {ordinal} {step}"))
                .map_err(|error| FlowError::failed(format!("the step log refused: {error}"))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// The clock of a world that never waits.
    fn clock() -> Clock {
        Clock::virtual_at(Duration::ZERO)
    }

    /// The same seed names the same plan, a different seed a different one, and
    /// a plan is a function of the seed alone (the double run, then the stranger).
    #[test]
    fn sim_one_seed_names_one_plan() -> TestResult {
        let world = |seed| {
            World::plan(seed, 400, FAILURE_PERCENT, Latencies::ZERO, clock(), None)
                .map_err(|error| error.line())
        };
        let (first, second, other) = (world(7)?, world(7)?, world(8)?);
        assert_eq!(first.plans(), second.plans());
        assert_ne!(first.plans(), other.plans());
        Ok(())
    }

    /// The draw is the rate the issue names: over many attempts the failing
    /// share sits near 20 percent, and no run's plan exceeds its attempts.
    #[test]
    fn sim_failure_share_is_near_the_named_rate() -> TestResult {
        let world = World::plan(3, 20_000, FAILURE_PERCENT, Latencies::ZERO, clock(), None)
            .map_err(|error| error.line())?;
        let (mut failed, mut tried) = (0_u64, 0_u64);
        for plan in world.plans() {
            for failing in [plan.outside, plan.model] {
                assert!(failing <= ATTEMPTS.get());
                let attempts = failing.saturating_add(1).min(ATTEMPTS.get());
                failed += u64::from(failing);
                tried += u64::from(attempts);
            }
        }
        let share = failed * 1_000 / tried;
        assert!((185..=215).contains(&share), "share {share} per mille");
        Ok(())
    }

    /// At zero percent nothing fails; at one hundred every attempt does.
    #[test]
    fn the_extreme_rates_fail_nothing_and_everything() -> TestResult {
        let none =
            World::plan(1, 50, 0, Latencies::ZERO, clock(), None).map_err(|error| error.line())?;
        assert!(
            none.plans()
                .iter()
                .all(|plan| plan.outside == 0 && plan.model == 0)
        );
        let all = World::plan(1, 50, 100, Latencies::ZERO, clock(), None)
            .map_err(|error| error.line())?;
        assert!(
            all.plans()
                .iter()
                .all(|plan| plan.outside == ATTEMPTS.get() && plan.model == ATTEMPTS.get())
        );
        Ok(())
    }

    /// A run outside the plan has no plan; the upstream refuses it typed.
    #[test]
    fn a_run_outside_the_plan_is_refused_not_invented() -> TestResult {
        let world = World::plan(1, 3, FAILURE_PERCENT, Latencies::ZERO, clock(), None)
            .map_err(|error| error.line())?;
        assert!(world.planned(2).is_some());
        assert!(world.planned(3).is_none());
        let refused = lgwks_bot::block_on(world.call(Call::Outside, 3, 1));
        assert!(
            matches!(refused, Err(FlowError::Failed { .. })),
            "{refused:?}"
        );
        Ok(())
    }

    /// A planned failure is a retryable domain error; the attempt after the
    /// planned ones succeeds with the reference value.
    #[test]
    fn a_planned_failure_is_retryable_and_then_the_value_arrives() -> TestResult {
        let world =
            World::plan(5, 1, 100, Latencies::ZERO, clock(), None).map_err(|error| error.line())?;
        for attempt in 1..=ATTEMPTS.get() {
            let refused = lgwks_bot::block_on(world.call(Call::Model, 0, attempt));
            let error = refused.err().ok_or("a planned failure succeeded")?;
            assert!(error.is_retryable(), "{error:?}");
        }
        let arrived = lgwks_bot::block_on(world.call(Call::Model, 0, ATTEMPTS.get() + 1))
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(arrived, Call::Model.value(0));
        assert_eq!(world.injected(), u64::from(ATTEMPTS.get()));
        Ok(())
    }

    /// An upstream call with latency is parked on the world's own clock: it is
    /// pending until the clock reaches its latency, one nanosecond short is still
    /// pending, and the advance onto it answers with the reference value.
    #[test]
    fn a_call_with_latency_waits_on_the_world_clock_and_not_on_real_time() -> TestResult {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let clock = clock();
        let world = World::plan(1, 1, 0, Latencies::WORKLOAD, clock.clone(), None)
            .map_err(|error| error.line())?;
        let latency = Latencies::WORKLOAD.model;
        let mut call = std::pin::pin!(world.call(Call::Model, 0, 1));
        let mut context = Context::from_waker(Waker::noop());
        assert!(call.as_mut().poll(&mut context).is_pending());
        assert!(clock.has_waiters(), "the call is not parked on the clock");
        let short = latency.saturating_sub(Duration::from_nanos(1));
        clock.advance(short).map_err(|error| error.to_string())?;
        assert!(
            call.as_mut().poll(&mut context).is_pending(),
            "one ns short answered"
        );
        clock
            .advance(Duration::from_nanos(1))
            .map_err(|error| error.to_string())?;
        match call.as_mut().poll(&mut context) {
            Poll::Ready(Ok(value)) => assert_eq!(value, Call::Model.value(0)),
            other => {
                lgwks_std::trace::debug!(?other, "the advance onto the latency did not answer");
                return Err(format!("the advance onto the latency gave {other:?}"));
            }
        }
        Ok(())
    }

    /// The reference model adds the seven weights, both upstream values and the
    /// payload bytes to the run number.
    #[test]
    fn the_reference_model_is_the_sum_of_its_parts() {
        assert_eq!(expected(0), 38 + 1 + 2 + PAYLOAD_BYTES);
        assert_eq!(expected(10), 10 + 38 + 31 + 52 + PAYLOAD_BYTES);
    }

    /// Whole lines from many writers never interleave.
    #[test]
    fn sim_many_writers_never_tear_a_line() -> TestResult {
        let dir = crate::workdir::Workdir::create(None).map_err(|error| error.line())?;
        let path = dir.path().join("steps.log");
        let log = AppendLog::open(&path, "test log").map_err(|error| error.line())?;
        std::thread::scope(|scope| {
            for writer in 0..8_u32 {
                let log = &log;
                scope.spawn(move || {
                    for step in 0..200_u32 {
                        if log.append(&format!("t{writer} {step} s1-intake")).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        assert_eq!(text.lines().count(), 8 * 200);
        assert!(text.lines().all(|line| line.split(' ').count() == 3));
        Ok(())
    }
}
