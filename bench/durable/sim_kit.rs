//! What the simulations share: a result type, a seeded dice, a trace hash, and a
//! bench of two tenants over run stores in a scratch directory with a step log.
//!
//! Nothing here judges. Every oracle lives in the test that uses the kit.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::rt::runtime::block_on;
use lgwks_bot::clock::Clock;
use lgwks_bot::rt::sync::CancellationToken;
use lgwks_std::seeded::Seeded;

use crate::error::BenchError;
use crate::fleet::{Finish, Fleet, Parking, Rig, Started, grant_and_resume, start};
use crate::workdir::Workdir;
use crate::world::{AppendLog, Latencies, World, expected};

/// A test's result: the message names what broke.
pub(crate) type TestResult = Result<(), String>;

/// A failure as the one line it would print.
pub(crate) fn line(error: BenchError) -> String {
    error.line()
}

/// Any displayable failure as a message.
pub(crate) fn text<E: Display>(error: E) -> String {
    error.to_string()
}

/// The clock a simulation's world waits on: virtual, so no wait costs real time.
/// A world with zero latencies never waits on it; one with latencies is run
/// under [`driven`].
pub(crate) fn sim_clock() -> Clock {
    Clock::virtual_at(Duration::ZERO)
}

/// How far the driver moves a virtual clock each time a sleep is parked on it:
/// past every latency the workload names, so one advance wakes every sleeper.
const DRIVER_STEP: Duration = Duration::from_secs(3_600);

/// Moves a virtual clock forward whenever a sleep is parked on it, until `stop`
/// is cancelled. The wait is the clock's own waiter signal, not a poll or a delay.
async fn drive(clock: &Clock, stop: &CancellationToken) {
    while stop
        .run_until_cancelled(clock.wait_for_waiter())
        .await
        .is_some()
    {
        if let Err(error) = clock.advance(DRIVER_STEP) {
            lgwks_std::trace::debug!(%error, "the driver could not advance the virtual clock");
        }
    }
}

/// Run `work` while a second thread drives the virtual `clock`, so every wait
/// the work makes on it ends without real time passing. The driver is stopped
/// when `work` returns or unwinds.
pub(crate) fn driven<T>(clock: &Clock, work: impl FnOnce() -> T) -> T {
    let stop = CancellationToken::new();
    let driver_stop = stop.child_token();
    std::thread::scope(|scope| {
        scope.spawn(|| block_on(drive(clock, &driver_stop)));
        // Dropped when `work` returns or unwinds, before the scope joins the driver.
        let _release = stop.drop_guard();
        work()
    })
}

/// A seeded stream whose refusals are messages.
pub(crate) struct Dice(Seeded);

impl Dice {
    /// The stream of `seed`.
    pub(crate) fn new(seed: u64) -> Self {
        Self(Seeded::from_seed(seed))
    }

    /// A number below `bound`.
    pub(crate) fn roll(&mut self, bound: u64) -> Result<u64, String> {
        self.0.below(bound).map_err(text)
    }

    /// A number below `bound`, as a `u32`.
    pub(crate) fn small(&mut self, bound: u32) -> Result<u32, String> {
        u32::try_from(self.roll(u64::from(bound))?).map_err(text)
    }

    /// A position below `bound`.
    pub(crate) fn pick(&mut self, bound: usize) -> Result<usize, String> {
        let wide = u64::try_from(bound).map_err(text)?;
        usize::try_from(self.roll(wide)?).map_err(text)
    }
}

/// A running hash of what a scenario saw, so two runs of one seed can be told to
/// agree with one number.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Trace(u64);

impl Trace {
    /// An empty trace.
    pub(crate) const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    /// Fold one observation in.
    pub(crate) fn note(&mut self, observation: &str) {
        for byte in observation.bytes().chain([0xff]) {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// The hash so far.
    pub(crate) const fn hash(self) -> u64 {
        self.0
    }
}

/// Two tenants' run stores and one step log, in a scratch directory.
pub(crate) struct Bench {
    /// The scratch directory, removed when the bench drops.
    pub(crate) dir: Workdir,
    /// The failure plan and the step log every fleet opened here shares.
    pub(crate) world: Arc<World>,
    /// The step log.
    pub(crate) log: PathBuf,
}

impl Bench {
    /// A bench planned for `runs` runs under `seed`, failing `percent` of attempts,
    /// with no waiting.
    pub(crate) fn new(seed: u64, runs: u32, percent: u64) -> Result<Self, String> {
        Self::with(seed, runs, percent, Latencies::ZERO)
    }

    /// As [`Bench::new`], with the given latencies.
    pub(crate) fn with(
        seed: u64,
        runs: u32,
        percent: u64,
        latencies: Latencies,
    ) -> Result<Self, String> {
        let dir = Workdir::create(None).map_err(line)?;
        let log = dir.path().join("exec.log");
        let steps = AppendLog::open(&log, "opening the step log").map_err(line)?;
        let world =
            World::plan(seed, runs, percent, latencies, sim_clock(), Some(steps)).map_err(line)?;
        Ok(Self {
            dir,
            world: Arc::new(world),
            log,
        })
    }

    /// Open both tenants' hosts over this bench's stores, as a restarted process would.
    pub(crate) fn open(&self) -> Result<Fleet, String> {
        Fleet::open(self.dir.path(), Arc::clone(&self.world)).map_err(line)
    }

    /// The run-store file of `tenant`.
    pub(crate) fn store(&self, tenant: &str) -> PathBuf {
        self.dir.path().join(format!("{tenant}.runstore"))
    }

    /// How many times each step body ran, keyed by its log line.
    pub(crate) fn step_counts(&self) -> Result<HashMap<String, u32>, String> {
        let log = std::fs::read_to_string(&self.log).map_err(text)?;
        let mut counts = HashMap::new();
        for row in log.lines().filter(|row| !row.trim().is_empty()) {
            let seen = counts.entry(row.to_owned()).or_insert(0_u32);
            *seen += 1;
        }
        Ok(counts)
    }

    /// Fail unless no step body ran more than `most` times.
    pub(crate) fn steps_at_most(&self, most: u32) -> Result<(), String> {
        for (row, seen) in self.step_counts()? {
            if seen > most {
                lgwks_std::trace::debug!(
                    row = row.as_str(),
                    seen,
                    most,
                    "a step body ran more often than allowed"
                );
                return Err(format!(
                    "step body {row:?} ran {seen} times, most is {most}"
                ));
            }
        }
        Ok(())
    }
}

/// A scenario that returns a trace, run twice on `seed`: the two traces must agree.
pub(crate) fn replays(scenario: fn(u64) -> Result<u64, String>, seed: u64) -> TestResult {
    let (first, second) = (scenario(seed)?, scenario(seed)?);
    if first == second {
        Ok(())
    } else {
        Err(format!(
            "seed {seed} replayed differently: {first:x} then {second:x}"
        ))
    }
}

/// Whether a resumed run carries the model's value; a different value is the refusal.
pub(crate) fn value_is_right(output: Option<&u64>, ordinal: u32) -> Result<bool, String> {
    match output {
        Some(got) if *got == expected(ordinal) => Ok(true),
        Some(got) => Err(format!(
            "run {ordinal} returned {got}, the model says {}",
            expected(ordinal)
        )),
        None => Ok(false),
    }
}

/// A digest no approval carries: sixty-four zero nibbles.
pub(crate) fn zero_digest() -> Result<lgwks_std::hash::Digest, String> {
    lgwks_std::hash::Digest::from_hex(&"0".repeat(64)).map_err(text)
}

/// Park run `ordinal` on `rig`.
pub(crate) fn parked(rig: &Rig, ordinal: u32) -> Result<Parking, BenchError> {
    match block_on(start(rig, ordinal))? {
        Started::Parked(parking) => Ok(parking),
        Started::Finished(finish) => Err(BenchError::RunFailed {
            ordinal,
            disposition: "finished",
            detail: format!("it did not park: {finish:?}"),
        }),
    }
}

/// Grant a parked run's approval at once and resume it to its end.
pub(crate) fn finish(rig: &Rig, ordinal: u32, parking: &Parking) -> Result<Finish, BenchError> {
    block_on(grant_and_resume(rig, ordinal, parking, Duration::ZERO))
}
