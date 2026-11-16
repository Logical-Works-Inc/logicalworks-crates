//! The flow measurement: drive `runs` runs over a bounded set of workers and
//! tally how each ended and how long each took, end to end.
//!
//! A run's latency is the whole of it: admission, ten durable steps, the park at
//! the approval, the wait for the decision, the decision and the resume. Runs the
//! crate's retry policy gave up are counted and left out of the latency sample,
//! never timed as if they were finished.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lgwks_bot::clock::Clock;

use crate::args::Args;
use crate::error::BenchError;
use crate::fleet::{
    Finish, Fleet, Loss, Rig, Started, TENANTS, WORKERS, grant_and_resume, pool, start,
};
use crate::report::{Durability, Envelope, FlowDoc, Workload, flow_code_lines};
use crate::stats::{Summary, micros, resident};
use crate::world::{AppendLog, FAILURE_PERCENT, Latencies, World};

/// What one measurement saw.
#[derive(Debug)]
pub(crate) struct Tally {
    /// Runs that finished all ten steps.
    pub(crate) finished: u32,
    /// End-to-end latency of each finished run, in microseconds.
    pub(crate) latencies_us: Vec<u64>,
    /// Runs the host's retry budget gave up.
    pub(crate) throttled: u64,
    /// Runs in which every attempt of a call failed.
    pub(crate) exhausted: u64,
    /// Retries taken.
    pub(crate) retries: u64,
    /// Failures injected.
    pub(crate) injected: u64,
    /// `sync_all` calls both stores made.
    pub(crate) fsyncs: u64,
    /// Records both stores staged.
    pub(crate) staged: u64,
    /// Wall time from the first run's start to the last run's end.
    pub(crate) elapsed: Duration,
    /// Microseconds, on the injected clock, from opening both tenants' hosts and
    /// run stores until the fleet could take its first run.
    pub(crate) deploy_us: u64,
}

/// How one timed run ended.
struct Sample {
    /// Microseconds from start to end.
    micros: u64,
    /// How it ended.
    finish: Finish,
}

/// Run `ordinal` to its end: start it, and if it parks, grant and resume it.
async fn complete(rig: &Rig, ordinal: u32, delay: Duration) -> Result<Finish, BenchError> {
    match start(rig, ordinal).await? {
        Started::Finished(finish) => Ok(finish),
        Started::Parked(parking) => grant_and_resume(rig, ordinal, &parking, delay).await,
    }
}

/// The flow measurement the command line asked for, as a report: the workload's
/// own failure rate and latencies, the run stores under `dir`, no step log.
///
/// # Errors
///
/// Any [`BenchError`] the measurement or the resident-set reading raises.
pub(crate) fn flow(args: &Args, dir: &Path, clock: Clock) -> Result<FlowDoc, BenchError> {
    let mut tally = measure(
        dir,
        args.runs,
        args.seed,
        FAILURE_PERCENT,
        Latencies::WORKLOAD,
        clock,
        None,
    )?;
    let memory = resident()?;
    let latency_us = Summary::of(&mut tally.latencies_us);
    let deploy_us = Summary::of(&mut [tally.deploy_us]);
    let seconds = tally.elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let per_second = f64::from(tally.finished) / seconds;
    Ok(FlowDoc {
        envelope: Envelope::new(&args.command, args.seed, args.runs)?,
        mode: "flow",
        tenants: TENANTS.len(),
        workers: WORKERS,
        durability: Durability::new(tally.fsyncs, tally.staged),
        workload: Workload::of(Latencies::WORKLOAD),
        samples: latency_us.samples,
        latency_us,
        deploy_us,
        throughput_runs_per_s: (per_second * 100.0).round() / 100.0,
        retries: tally.retries,
        failures_injected: tally.injected,
        throttled_runs: tally.throttled,
        exhausted_runs: tally.exhausted,
        peak_rss_bytes: memory.bytes,
        peak_rss_source: memory.source,
        flow_code_lines: flow_code_lines(),
    })
}

/// Measure `runs` runs under `seed` against run stores under `store_dir`.
///
/// `percent` is the share of attempts the upstreams fail; the workload is 20.
/// Every upstream wait and every approval delay is made on `clock`.
///
/// # Errors
///
/// Any [`BenchError`] a run, the plan or a store raises: a run that ends in a
/// way the workload does not name stops the measurement.
pub(crate) fn measure(
    store_dir: &Path,
    runs: u32,
    seed: u64,
    percent: u64,
    latencies: Latencies,
    clock: Clock,
    steps: Option<AppendLog>,
) -> Result<Tally, BenchError> {
    let world = Arc::new(World::plan(seed, runs, percent, latencies, clock.clone(), steps)?);
    // The deploy is the fleet's start to ready: both tenants' hosts and run
    // stores open, and nothing is planned or run yet. It is read off the injected
    // clock, the one every wait of the run is made on.
    let deploying = clock.now();
    let fleet = Fleet::open(store_dir, Arc::clone(&world))?;
    let deploy_us = micros(clock.now().saturating_sub(deploying));
    let delay = world.latencies().approval;
    let started = Instant::now();
    let samples = pool(runs, WORKERS, |ordinal| {
        let at = Instant::now();
        let finish = lgwks_bot::rt::runtime::block_on(complete(fleet.rig(ordinal), ordinal, delay))?;
        Ok(Sample {
            micros: micros(at.elapsed()),
            finish,
        })
    })?;
    let elapsed = started.elapsed();
    let (mut latencies_us, mut throttled, mut exhausted) = (Vec::new(), 0, 0);
    let mut finished = 0_u32;
    for sample in samples {
        match sample.finish {
            Finish::Done => {
                latencies_us.push(sample.micros);
                finished += 1;
            }
            Finish::Lost(Loss::Throttled) => throttled += 1,
            Finish::Lost(Loss::Exhausted) => exhausted += 1,
        }
    }
    let (fsyncs, staged) = fleet.flush_counts();
    Ok(Tally {
        finished,
        latencies_us,
        throttled,
        exhausted,
        retries: world.retries(),
        injected: world.injected(),
        fsyncs,
        staged,
        elapsed,
        deploy_us,
    })
}
