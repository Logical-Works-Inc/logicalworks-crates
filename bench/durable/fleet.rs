//! The two tenants' hosts, and the doors a run goes through: start it, grant
//! its approval, resume it, and spread many runs over a bounded set of workers.
//!
//! Each tenant has its own host, its own admission, its own retry budget and its
//! own run-store file under one directory, so nothing here is shared between
//! tenants but the process and the directory.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use lgwks_bot::clock::Clock;
use lgwks_bot::effect::RunId;
use lgwks_bot::script::{ApprovalDecision, FlowError, PendingApproval};
use lgwks_bot::task::{Disposition, Host, Report, RunStore};
use lgwks_std::hash::Digest;

use crate::error::BenchError;
use crate::flow::{FlowTask, flow_task};
use crate::world::{World, expected};

/// The tenants, in the order a run's ordinal picks them.
pub(crate) const TENANTS: [&str; 2] = ["alpha", "beta"];

/// Runs in flight at once: one thread per worker, each driving one run.
pub(crate) const WORKERS: u32 = 64;

/// The tenant run `ordinal` belongs to: even runs to alpha, odd to beta, so the
/// split is even and one tenant never runs another's ordinal.
pub(crate) fn tenant_of(ordinal: u32) -> &'static str {
    let [alpha, beta] = TENANTS;
    if ordinal.is_multiple_of(2) {
        alpha
    } else {
        beta
    }
}

/// Who the benchmark's operator is on a recorded decision.
const OPERATOR: &str = "bench-operator";

/// One tenant's host and the task it runs.
pub(crate) struct Rig {
    /// The tenant's host.
    pub(crate) host: Host,
    /// The flow, over the shared world.
    pub(crate) task: FlowTask,
    /// The clock a human's decision delay is waited on: the world's own.
    pub(crate) clock: Clock,
}

/// Why a run did not finish its ten steps, when the crate's own policy ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Loss {
    /// The host's retry budget was spent, so no further retry was made.
    Throttled,
    /// Every attempt of a call failed.
    Exhausted,
}

/// How one run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Finish {
    /// All ten steps ran and the value is the model's.
    Done,
    /// The crate's retry policy gave the run up.
    Lost(Loss),
}

/// A run parked at its approval: what a decision and a resume need.
#[derive(Debug, Clone)]
pub(crate) struct Parking {
    /// The run.
    pub(crate) run: RunId,
    /// The approval's id.
    pub(crate) id: String,
    /// The digest a decision must name.
    pub(crate) digest: Digest,
}

/// What starting a run came to.
#[derive(Debug)]
pub(crate) enum Started {
    /// It ran to the end without parking.
    Finished(Finish),
    /// It parked at its approval.
    Parked(Parking),
}

/// Both tenants' rigs over one world.
pub(crate) struct Fleet {
    /// Alpha, then beta.
    rigs: [Rig; 2],
}

impl Fleet {
    /// Open both tenants' hosts over run stores under `store_dir`.
    ///
    /// # Errors
    ///
    /// [`BenchError::Flow`], [`BenchError::Store`] or [`BenchError::Host`] with
    /// the stage that refused.
    pub(crate) fn open(store_dir: &Path, world: Arc<World>) -> Result<Self, BenchError> {
        let rig = |tenant: &str| -> Result<Rig, BenchError> {
            let host = Host::builder(tenant)
                .map_err(|source| BenchError::Flow {
                    stage: "host builder",
                    source,
                })?
                .run_store(store_dir)
                .map_err(|source| BenchError::Store {
                    stage: "opening the run store",
                    source,
                })?
                .build()
                .map_err(|source| BenchError::Host {
                    stage: "build",
                    source,
                })?;
            let task = flow_task(Arc::clone(&world)).map_err(|source| BenchError::Flow {
                stage: "flow task",
                source,
            })?;
            Ok(Rig {
                host,
                task,
                clock: world.clock().clone(),
            })
        };
        let [alpha, beta] = TENANTS;
        Ok(Self {
            rigs: [rig(alpha)?, rig(beta)?],
        })
    }

    /// The rig run `ordinal` belongs to, the one of [`tenant_of`].
    pub(crate) fn rig(&self, ordinal: u32) -> &Rig {
        let [alpha, beta] = &self.rigs;
        if ordinal.is_multiple_of(2) {
            alpha
        } else {
            beta
        }
    }

    /// Both rigs, in tenant order.
    pub(crate) fn rigs(&self) -> &[Rig; 2] {
        &self.rigs
    }

    /// The fsyncs and staged records of both stores.
    pub(crate) fn flush_counts(&self) -> (u64, u64) {
        self.rigs
            .iter()
            .filter_map(|rig| rig.host.run_store().map(RunStore::flush_counts))
            .fold((0, 0), |(syncs, staged), (more_syncs, more_staged)| {
                (
                    syncs.saturating_add(more_syncs),
                    staged.saturating_add(more_staged),
                )
            })
    }
}

/// A run that ended some way the workload does not expect.
fn failed(ordinal: u32, report: &Report<u64>) -> BenchError {
    let detail = match report.error() {
        Some(error) => error.to_string(),
        None => "no error was reported".to_owned(),
    };
    BenchError::RunFailed {
        ordinal,
        disposition: report.disposition().label(),
        detail,
    }
}

/// A run's value, checked against the reference model.
fn checked(ordinal: u32, report: &Report<u64>) -> Result<(), BenchError> {
    match report.output() {
        Some(got) if *got == expected(ordinal) => Ok(()),
        Some(got) => Err(BenchError::WrongOutput {
            ordinal,
            want: expected(ordinal),
            got: *got,
        }),
        None => Err(failed(ordinal, report)),
    }
}

/// Start run `ordinal`: it finishes, parks at its approval, or is given up by
/// the retry policy.
///
/// # Errors
///
/// A [`BenchError`] for any other ending: that is a defect in the run, not a
/// measurement.
pub(crate) async fn start(rig: &Rig, ordinal: u32) -> Result<Started, BenchError> {
    let report = rig.host.run(&rig.task, ordinal).await;
    if report.disposition().is_success() {
        checked(ordinal, &report)?;
        return Ok(Started::Finished(Finish::Done));
    }
    match (report.disposition(), report.error()) {
        (Disposition::Parked, Some(FlowError::Parked { id, digest, .. })) => {
            let run = report.run_id().ok_or_else(|| failed(ordinal, &report))?;
            let digest =
                Digest::from_hex(digest).map_err(|source| BenchError::Digest { source })?;
            Ok(Started::Parked(Parking {
                run,
                id: id.clone(),
                digest,
            }))
        }
        (_, Some(FlowError::Throttled { .. })) => {
            Ok(Started::Finished(Finish::Lost(Loss::Throttled)))
        }
        (_, Some(FlowError::Exhausted { .. })) => {
            Ok(Started::Finished(Finish::Lost(Loss::Exhausted)))
        }
        _ => Err(failed(ordinal, &report)),
    }
}

/// Grant a parked approval after `delay` on the rig's clock, then resume the run
/// to its end.
///
/// # Errors
///
/// [`BenchError::Decision`] when the decision is refused; any other ending of
/// the resumed run is a [`BenchError`].
pub(crate) async fn grant_and_resume(
    rig: &Rig,
    ordinal: u32,
    parking: &Parking,
    delay: Duration,
) -> Result<Finish, BenchError> {
    if !delay.is_zero() {
        rig.clock.sleep(delay).await;
    }
    rig.host
        .decide(
            &parking.id,
            ApprovalDecision::approve(OPERATOR, parking.digest),
        )
        .map_err(|source| BenchError::Decision { source })?;
    let report = rig.host.resume(parking.run, &rig.task, ordinal).await;
    checked(ordinal, &report)?;
    Ok(Finish::Done)
}

/// The parking a host's pending listing names.
pub(crate) fn parking_of(pending: &PendingApproval) -> Parking {
    Parking {
        run: pending.run(),
        id: pending.id().to_owned(),
        digest: *pending.digest(),
    }
}

/// Run `job` once for each of `items` over at most `workers` threads, as [`pool`]
/// does for a count.
///
/// # Errors
///
/// As [`pool`], and [`BenchError::OutsideJobList`] if a job number names no item.
pub(crate) fn pool_over<I, T, F>(items: &[I], workers: u32, job: F) -> Result<Vec<T>, BenchError>
where
    I: Sync,
    T: Send,
    F: Fn(&I) -> Result<T, BenchError> + Sync,
{
    let total = u32::try_from(items.len()).map_err(|source| {
        lgwks_std::trace::debug!(items = items.len(), %source, "the job list is longer than a job number");
        BenchError::OutsideJobList { index: u32::MAX }
    })?;
    pool(total, workers, |index| {
        let item = usize::try_from(index).ok().and_then(|at| items.get(at));
        match item {
            Some(found) => job(found),
            None => Err(BenchError::OutsideJobList { index }),
        }
    })
}

/// The text a panic carried: a `&str` or a `String` payload, else a note that it
/// carried neither.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "the panic carried no text".to_owned()
}

/// Run `total` jobs over at most `workers` threads, each thread pulling the next
/// job number from one counter: no queue exists to overflow, and at most
/// `workers` jobs are ever in flight. The first failure stops the others from
/// taking a new job.
///
/// # Errors
///
/// The first job failure, [`BenchError::Io`] when a thread cannot start and
/// [`BenchError::WorkerPanicked`] when one unwinds.
pub(crate) fn pool<T, F>(total: u32, workers: u32, job: F) -> Result<Vec<T>, BenchError>
where
    T: Send,
    F: Fn(u32) -> Result<T, BenchError> + Sync,
{
    let next = AtomicU32::new(0);
    let stop = AtomicBool::new(false);
    let threads = workers.clamp(1, total.max(1));
    let outcomes = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..threads {
            let (next, stop, job) = (&next, &stop, &job);
            let spawned = std::thread::Builder::new()
                .name(format!("durable-worker-{index}"))
                .spawn_scoped(scope, move || -> Result<Vec<T>, BenchError> {
                    let mut done = Vec::new();
                    while !stop.load(Ordering::Relaxed) {
                        let ordinal = next.fetch_add(1, Ordering::Relaxed);
                        if ordinal >= total {
                            break;
                        }
                        match job(ordinal) {
                            Ok(value) => done.push(value),
                            Err(error) => {
                                stop.store(true, Ordering::Relaxed);
                                lgwks_std::trace::debug!(%error, ordinal, "a job failed; the other workers stop");
                                return Err(error);
                            }
                        }
                    }
                    Ok(done)
                });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(source) => {
                    stop.store(true, Ordering::Relaxed);
                    lgwks_std::trace::debug!(%source, index, "a worker thread could not start");
                    return Err(BenchError::Io {
                        context: "starting a worker thread",
                        source,
                    });
                }
            }
        }
        let mut outcomes = Vec::new();
        for handle in handles {
            outcomes.push(handle.join().map_err(|payload| {
                let message = panic_message(payload.as_ref());
                lgwks_std::trace::debug!(%message, "a worker thread panicked");
                BenchError::WorkerPanicked { message }
            }));
        }
        Ok(outcomes)
    })?;
    let mut all = Vec::new();
    for outcome in outcomes {
        all.extend(outcome??);
    }
    Ok(all)
}
