//! The Restate row: the same ten-step flow, on `restate-server` and a worker
//! process, driven over HTTP and timed from the outside.
//!
//! The driver owns the whole stack. It picks the ports by binding, writes the
//! server's config into the work directory, refuses to go on unless the server's
//! own `--dump-config` says the log is written ahead and fsynced, starts the
//! server and the worker as children that are killed and reaped on every exit
//! path, registers the worker, and then does what a user does: submit a run,
//! grant its approval once the worker says it parked and the delay has passed,
//! and wait for it to end. The failure plan is the one the lgwks row draws from
//! the seed, handed to the worker as a file, so both rows fail the same attempts.
//!
//! Restate has no tenancy. A run's workflow key is `<tenant>-<run>`, and the row
//! says so. Recovery is `kill -9` of the server with runs parked at their
//! approval, a restart on the same base directory, every approval granted again
//! and the clock stopped when the last run is done; the worker lives through it.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lgwks_bot::clock::Clock;

use crate::args::{Args, Row};
use crate::error::{BenchError, ChildCause};
use crate::fleet::{Finish, Loss, TENANTS, WORKERS, pool, pool_over, tenant_of};
use crate::measure;
use crate::recover::{check_steps, recover as recover_lgwks};
use crate::report::{Envelope, Recovery, RowDoc, TableDoc, Workload, code_lines};
use crate::rival_config::{Durability, check_durability, config_text, pick_ports};
use crate::rival_error::RivalError;
use crate::rival_http::{Attached, Wire, until};
use crate::rival_proc::{Notices, Outcome, Owned, pump, resident_of};
use crate::rival_wire::{key_of, parse_key, parse_plan};
use crate::stats::{Summary, micros};
use crate::world::{FAILURE_PERCENT, Latencies, World, expected};

/// The Restate worker's own source, counted at build time for the code-size line.
const FLOW_SOURCE: &str = include_str!("../restate/flow.rs");

/// The manifest, read at build time so the row names the SDK it was built with.
const MANIFEST: &str = include_str!("../Cargo.toml");

/// The worker binary, built beside this one.
const WORKER: &str = "lgwks-bench-restate-worker";

/// What the row's tenancy line says.
const TENANCY: &str = "none: one workflow per run, keyed <tenant>-<run>";

/// What the peak resident set covers on this row.
const RSS_COVERS: &str = "restate-server plus the worker, ps after the run";

/// Tries a door gets to answer healthy, a try every [`GAP`].
const HEALTH_TRIES: u32 = 600;

/// Tries the worker's registration gets.
const REGISTER_TRIES: u32 = 200;

/// Tries one approval gets after a restart.
const APPROVE_TRIES: u32 = 400;

/// The wait between two tries.
const GAP: Duration = Duration::from_millis(50);

/// The longest a run may take to park or be lost.
const RUN_BOUND: Duration = Duration::from_secs(120);

/// The files the stack leaves in its directory.
const PLAN_FILE: &str = "plan.txt";
/// The step log: one line per step body that ran.
const STEP_LOG: &str = "exec.log";
/// The attempt log: one line per retry and per injected failure.
const ATTEMPT_LOG: &str = "attempts.log";

/// How one timed run ended.
struct Sample {
    /// Microseconds from submit to end.
    micros: u64,
    /// How it ended.
    finish: Finish,
}

/// The version a dependency value pins with `=`: a bare string
/// (`"=1.2.3"`) or the `version` key of an inline table (`{ version = "=1.2.3", .. }`).
fn pinned(value: &str) -> Option<&str> {
    let quoted = match value.strip_prefix('{') {
        Some(table) => table.split_once("version = ")?.1,
        None => value,
    };
    let (version, _) = quoted.strip_prefix("\"=")?.split_once('"')?;
    Some(version)
}

/// The SDK version the manifest pins.
pub(crate) fn sdk_version(manifest: &str) -> Result<String, BenchError> {
    manifest
        .lines()
        .filter_map(|line| line.trim().strip_prefix("restate-sdk = "))
        .find_map(pinned)
        .map(str::to_owned)
        .ok_or_else(|| {
            lgwks_std::trace::debug!("the manifest does not pin restate-sdk");
            BenchError::MalformedRecord {
                file: "bench/Cargo.toml",
                line: "no restate-sdk pin".to_owned(),
            }
        })
}

/// The `(retries, injected failures)` an attempt log records: a line is
/// `<key> <step> <attempt> <ok|failed>`, a retry is any attempt past the first.
///
/// # Errors
///
/// [`RivalError::WorkerRecord`] for a line that does not read.
pub(crate) fn parse_attempts(text: &str) -> Result<(u64, u64), RivalError> {
    let (mut retries, mut injected) = (0_u64, 0_u64);
    for line in text.lines() {
        let bad = || RivalError::WorkerRecord {
            file: ATTEMPT_LOG,
            line: line.to_owned(),
        };
        let words: Vec<&str> = line.split(' ').collect();
        let [key, step, attempt, word] = words[..] else {
            lgwks_std::trace::debug!(line, "an attempt line does not have four words");
            return Err(bad());
        };
        let attempt: u32 = attempt.parse().map_err(|source| {
            lgwks_std::trace::debug!(%source, line, "an attempt number is not a number");
            bad()
        })?;
        let failed = match word {
            "failed" => true,
            "ok" => false,
            other => {
                lgwks_std::trace::debug!(line, word = other, "an attempt line's last word is neither ok nor failed");
                return Err(bad());
            }
        };
        if parse_key(key).is_none() || step.is_empty() || attempt == 0 || (!failed && attempt < 2) {
            lgwks_std::trace::debug!(line, "an attempt line contradicts itself");
            return Err(bad());
        }
        retries += u64::from(attempt >= 2);
        injected += u64::from(failed);
    }
    Ok((retries, injected))
}

/// The version line of `server`.
fn server_version(server: &str) -> Result<String, BenchError> {
    let output = Command::new(server)
        .arg("--version")
        .output()
        .map_err(|source| BenchError::Rival {
            source: RivalError::Spawn {
                what: "restate-server --version",
                source,
            },
        })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The failing attempts of every run, from the plan the lgwks row draws.
fn plan_of(world: &World, runs: u32) -> Result<Vec<(u32, u32)>, BenchError> {
    (0..runs)
        .map(|ordinal| {
            world
                .planned(ordinal)
                .map(|faults| (faults.outside, faults.model))
                .ok_or(BenchError::OutsideJobList { index: ordinal })
        })
        .collect()
}

/// Make `dir` the stack's directory and write `text` to `name` in it.
fn write_file(dir: &Path, name: &str, text: &str) -> Result<PathBuf, BenchError> {
    let path = dir.join(name);
    std::fs::write(&path, text).map_err(|source| BenchError::Io {
        context: "writing a file of the Restate stack",
        source,
    })?;
    Ok(path)
}

/// A log file of the stack, for a child's stdout and stderr.
fn log_file(dir: &Path, name: &str) -> Result<File, BenchError> {
    File::create(dir.join(name)).map_err(|source| BenchError::Io {
        context: "creating a log file of the Restate stack",
        source,
    })
}

/// The running stack: server, worker and the thread that reads the worker.
struct Rig {
    /// The doors of the server.
    wire: Wire,
    /// What the worker has said.
    notices: Arc<Notices>,
    /// The server's name or path, as the user gave it.
    server_name: String,
    /// The config file the server runs under.
    config: PathBuf,
    /// The directory the stack lives in.
    dir: PathBuf,
    /// The server.
    server: Owned,
    /// The worker.
    worker: Owned,
    /// The thread reading the worker's stdout.
    reader: Option<JoinHandle<Result<(), RivalError>>>,
    /// What the server will flush.
    durability: Durability,
    /// The clock every wait is made on.
    clock: Clock,
}

/// Start the server on `config`, its output in `dir`.
fn spawn_server(name: &str, config: &Path, dir: &Path) -> Result<Owned, BenchError> {
    let out = log_file(dir, "server.log")?;
    let err = out.try_clone().map_err(|source| BenchError::Io {
        context: "sharing the server log",
        source,
    })?;
    Owned::spawn(
        "restate-server",
        Command::new(name)
            .arg("--config-file")
            .arg(config)
            .arg("--no-logo")
            .args(["--listen-mode", "tcp"])
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err),
    )
}

impl Rig {
    /// Wait for both doors of the server.
    fn await_health(&self) -> Result<(), BenchError> {
        let pause = |gap| lgwks_bot::block_on(self.clock.sleep(gap));
        until("the server's health", HEALTH_TRIES, GAP, &pause, || {
            self.wire.healthy()
        })
        .map_err(|source| BenchError::Rival { source })
    }

    /// Start the stack in `dir` over `plan`, recording step bodies if asked.
    fn start(
        args: &Args,
        dir: &Path,
        plan: &[(u32, u32)],
        steps: bool,
        clock: &Clock,
    ) -> Result<Self, BenchError> {
        let ports = pick_ports()?;
        let text: String = plan
            .iter()
            .map(|(outside, model)| format!("{outside} {model}\n"))
            .collect();
        let plan_file = write_file(dir, PLAN_FILE, &text)?;
        if parse_plan(&text).as_deref() != Some(plan) {
            lgwks_std::trace::debug!("the plan the worker will read is not the plan drawn");
            return Err(BenchError::Rival {
                source: RivalError::WorkerRecord {
                    file: PLAN_FILE,
                    line: "the plan does not read back".to_owned(),
                },
            });
        }
        let base = dir.join("restate-data");
        let config_body =
            config_text(&base, &ports).map_err(|source| BenchError::Rival { source })?;
        let config = write_file(dir, "restate.toml", &config_body)?;
        let durability = check_durability(&args.restate_server, &config)?;
        let server = spawn_server(&args.restate_server, &config, dir)?;
        let mut command = Command::new(worker_path()?);
        command
            .arg("--port")
            .arg(ports.worker.to_string())
            .arg("--plan")
            .arg(&plan_file)
            .arg("--attempts")
            .arg(dir.join(ATTEMPT_LOG));
        if steps {
            command.arg("--steps").arg(dir.join(STEP_LOG));
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log_file(dir, "worker.log")?);
        let mut worker = Owned::spawn("the Restate worker", &mut command)?;
        let notices = Arc::new(Notices::default());
        let reader = spawn_reader(&mut worker, &notices)?;
        let rig = Self {
            wire: Wire::new(&ports)?,
            notices,
            server_name: args.restate_server.clone(),
            config,
            dir: dir.to_path_buf(),
            server,
            worker,
            reader: Some(reader),
            durability,
            clock: clock.clone(),
        };
        rig.await_health()?;
        let pause = |gap| lgwks_bot::block_on(rig.clock.sleep(gap));
        until("registering the worker", REGISTER_TRIES, GAP, &pause, || {
            rig.wire.register().map(|()| true)
        })
        .map_err(|source| BenchError::Rival { source })?;
        Ok(rig)
    }

    /// `kill -9` the server, reap it, start it again on the same base directory
    /// and wait until both doors answer.
    fn crash_and_restart(&mut self) -> Result<(), BenchError> {
        self.server.kill_and_reap()?;
        self.server = spawn_server(&self.server_name, &self.config, &self.dir)?;
        self.await_health()
    }

    /// The retries and injected failures the worker logged.
    fn attempt_counts(&self) -> Result<(u64, u64), BenchError> {
        let text = std::fs::read_to_string(self.dir.join(ATTEMPT_LOG)).map_err(|source| {
            BenchError::Io {
                context: "reading the attempt log",
                source,
            }
        })?;
        parse_attempts(&text).map_err(|source| BenchError::Rival { source })
    }

    /// The resident set of the server and the worker together, in bytes.
    fn resident(&self) -> Result<u64, BenchError> {
        let (Some(server), Some(worker)) = (self.server.id(), self.worker.id()) else {
            lgwks_std::trace::debug!("a process of the stack is gone before its memory is read");
            return Err(BenchError::MemoryReading {
                reason: "a process of the Restate stack is gone",
            });
        };
        Ok(resident_of(server)?.saturating_add(resident_of(worker)?))
    }
}

impl Drop for Rig {
    /// Kill both children, then join the reader, which ends when the worker's
    /// stdout closes.
    fn drop(&mut self) {
        for owned in [&mut self.worker, &mut self.server] {
            if let Err(error) = owned.kill_and_reap() {
                lgwks_std::trace::warn!(%error, "a child of the Restate stack was not reaped");
            }
        }
        if let Some(reader) = self.reader.take() {
            match reader.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    lgwks_std::trace::warn!(%error, "the worker's notices ended with an error");
                }
                Err(_) => lgwks_std::trace::warn!("the notice reader panicked"),
            }
        }
    }
}

/// The worker binary beside this executable.
fn worker_path() -> Result<PathBuf, BenchError> {
    let exe = std::env::current_exe().map_err(|source| BenchError::Io {
        context: "finding this executable",
        source,
    })?;
    Ok(exe.with_file_name(format!("{WORKER}{}", std::env::consts::EXE_SUFFIX)))
}

/// Start the thread that reads `worker`'s stdout into `notices`.
fn spawn_reader(
    worker: &mut Owned,
    notices: &Arc<Notices>,
) -> Result<JoinHandle<Result<(), RivalError>>, BenchError> {
    let Some(stdout) = worker.take_stdout() else {
        lgwks_std::trace::debug!("the worker has no stdout pipe");
        return Err(BenchError::Io {
            context: "taking the worker's stdout",
            source: std::io::Error::other("the pipe was not made"),
        });
    };
    let notices = Arc::clone(notices);
    std::thread::Builder::new()
        .name("lgwks-bench-restate-notices".to_owned())
        .spawn(move || pump(stdout, &notices))
        .map_err(|source| BenchError::Io {
            context: "starting the notice reader",
            source,
        })
}

/// Submit run `ordinal` and wait for the worker to say it parked or was lost.
fn submit(rig: &Rig, ordinal: u32) -> Result<(String, Outcome), BenchError> {
    let key = key_of(tenant_of(ordinal), ordinal);
    rig.wire
        .submit(&key, ordinal)
        .map_err(|source| BenchError::Rival { source })?;
    let outcome = rig
        .notices
        .wait_for(&key, RUN_BOUND)
        .map_err(|source| BenchError::Rival { source })?;
    Ok((key, outcome))
}

/// Wait for a finished run and check its value against the reference model.
fn finish_ok(rig: &Rig, key: &str, ordinal: u32) -> Result<(), BenchError> {
    match rig
        .wire
        .attach(key)
        .map_err(|source| BenchError::Rival { source })?
    {
        Attached::Done(value) if value == expected(ordinal) => Ok(()),
        Attached::Done(got) => Err(BenchError::WrongOutput {
            ordinal,
            want: expected(ordinal),
            got,
        }),
        Attached::Failed { message, .. } => Err(BenchError::RunFailed {
            ordinal,
            disposition: "failed",
            detail: message,
        }),
    }
}

/// One timed run: submit, wait to park, wait the decision's delay, approve, wait
/// for the end. A run the retry policy gave up is a loss, as on the lgwks row.
fn one_run(rig: &Rig, ordinal: u32, delay: Duration) -> Result<Sample, BenchError> {
    let at = Instant::now();
    let (key, outcome) = submit(rig, ordinal)?;
    let finish = match outcome {
        Outcome::Parked => {
            lgwks_bot::block_on(rig.clock.sleep(delay));
            rig.wire
                .approve(&key, ordinal)
                .map_err(|source| BenchError::Rival { source })?;
            finish_ok(rig, &key, ordinal)?;
            Finish::Done
        }
        Outcome::Lost => {
            let attached = rig
                .wire
                .attach(&key)
                .map_err(|source| BenchError::Rival { source })?;
            if !matches!(attached, Attached::Failed { .. }) {
                lgwks_std::trace::debug!(key = key.as_str(), "a lost run attached as finished");
                return Err(BenchError::Rival {
                    source: RivalError::Inconsistent {
                        key,
                        why: "the worker said it was lost and the server says it finished",
                    },
                });
            }
            Finish::Lost(Loss::Exhausted)
        }
    };
    Ok(Sample {
        micros: micros(at.elapsed()),
        finish,
    })
}

/// The row an engine's measurement is reported as.
fn row_of(args: &Args, parts: RowParts) -> Result<RowDoc, BenchError> {
    Ok(RowDoc {
        row: "restate",
        engine: format!(
            "{} + restate-sdk {}",
            server_version(&args.restate_server)?,
            sdk_version(MANIFEST)?
        ),
        tenancy: TENANCY.to_owned(),
        durability: parts.durability,
        rss_covers: RSS_COVERS,
        runs: args.runs,
        samples: parts.latency.samples,
        latency_us: parts.latency,
        throughput_runs_per_s: parts.throughput,
        peak_rss_bytes: parts.rss,
        retries: parts.retries,
        failures_injected: parts.injected,
        throttled_runs: 0,
        exhausted_runs: parts.exhausted,
        code_lines: code_lines(FLOW_SOURCE),
        cold_build: None,
        recovery: None,
    })
}

/// What the flow measurement hands to [`row_of`].
struct RowParts {
    /// The durability line.
    durability: String,
    /// End-to-end latency of the finished runs.
    latency: Summary,
    /// Finished runs per second.
    throughput: f64,
    /// Resident set of both processes.
    rss: u64,
    /// Retries taken.
    retries: u64,
    /// Failures injected.
    injected: u64,
    /// Runs lost to the retry policy.
    exhausted: u64,
}

/// The Restate row of the flow measurement, with the stack in `dir`.
///
/// # Errors
///
/// Any [`BenchError`] the stack, a run or the memory reading raises.
pub(crate) fn flow(args: &Args, dir: &Path, clock: &Clock) -> Result<RowDoc, BenchError> {
    let world = World::plan(
        args.seed,
        args.runs,
        FAILURE_PERCENT,
        Latencies::WORKLOAD,
        clock.clone(),
        None,
    )?;
    let plan = plan_of(&world, args.runs)?;
    let rig = Rig::start(args, dir, &plan, false, clock)?;
    let delay = Latencies::WORKLOAD.approval;
    let started = Instant::now();
    let samples = pool(args.runs, WORKERS, |ordinal| one_run(&rig, ordinal, delay))?;
    let elapsed = started.elapsed();
    let (mut latencies, mut exhausted, mut finished) = (Vec::new(), 0_u64, 0_u32);
    for sample in samples {
        match sample.finish {
            Finish::Done => {
                latencies.push(sample.micros);
                finished += 1;
            }
            Finish::Lost(_) => exhausted += 1,
        }
    }
    let latency = Summary::of(&mut latencies);
    let (retries, injected) = rig.attempt_counts()?;
    let seconds = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let throughput = (f64::from(finished) / seconds * 100.0).round() / 100.0;
    let parts = RowParts {
        durability: rig.durability.line(),
        latency,
        throughput,
        rss: rig.resident()?,
        retries,
        injected,
        exhausted,
    };
    row_of(args, parts)
}

/// One run to decide after the restart.
struct Parked {
    /// The run.
    ordinal: u32,
    /// Its workflow key.
    key: String,
}

/// One round: park, kill the server, restart it, approve everything, check.
fn round(index: u32, args: &Args, root: &Path, clock: &Clock) -> Result<(u32, u64, u64), BenchError> {
    let dir = root.join(format!("round-{index}"));
    std::fs::create_dir(&dir).map_err(|source| BenchError::Io {
        context: "making the round directory",
        source,
    })?;
    let seed = args.seed.wrapping_add(u64::from(index));
    let world = World::plan(
        seed,
        args.runs,
        FAILURE_PERCENT,
        Latencies::WORKLOAD,
        clock.clone(),
        None,
    )?;
    let mut rig = Rig::start(args, &dir, &plan_of(&world, args.runs)?, true, clock)?;
    let ends = pool(args.runs, WORKERS, |ordinal| {
        let (key, outcome) = submit(&rig, ordinal)?;
        Ok((Parked { ordinal, key }, outcome))
    })?;
    let parked: Vec<Parked> = ends
        .into_iter()
        .filter_map(|(run, outcome)| (outcome == Outcome::Parked).then_some(run))
        .collect();
    if parked.is_empty() {
        lgwks_std::trace::debug!(runs = args.runs, "every run ended before it parked");
        return Err(BenchError::ChildFailed {
            cause: ChildCause::NothingParked,
        });
    }
    let clock_start = Instant::now();
    rig.crash_and_restart()?;
    let pause = |gap| lgwks_bot::block_on(rig.clock.sleep(gap));
    pool_over(&parked, WORKERS, |run| {
        until("an approval after the restart", APPROVE_TRIES, GAP, &pause, || {
            rig.wire.approve(&run.key, run.ordinal).map(|()| true)
        })
        .map_err(|source| BenchError::Rival { source })?;
        finish_ok(&rig, &run.key, run.ordinal)
    })?;
    let took = micros(clock_start.elapsed());
    let log = std::fs::read_to_string(dir.join(STEP_LOG)).map_err(|source| BenchError::Io {
        context: "reading the step log",
        source,
    })?;
    let recovered: Vec<(&'static str, u32)> = parked
        .iter()
        .map(|run| (tenant_of(run.ordinal), run.ordinal))
        .collect();
    let checked = check_steps(&log, &recovered)?;
    let count = u32::try_from(parked.len()).map_err(|source| {
        lgwks_std::trace::debug!(%source, "more parked runs than a count holds");
        BenchError::OutsideJobList { index: u32::MAX }
    })?;
    Ok((count, took, checked))
}

/// The Restate row's kill -9 recovery over `args.rounds` rounds under `root`.
///
/// # Errors
///
/// The first [`BenchError`] a round raises, or the one `progress` returns.
pub(crate) fn recover(
    args: &Args,
    root: &Path,
    clock: &Clock,
    progress: &mut dyn FnMut(&str) -> Result<(), BenchError>,
) -> Result<Recovery, BenchError> {
    let (mut parked_per_round, mut per_round, mut steps_checked) = (Vec::new(), Vec::new(), 0_u64);
    for index in 0..args.rounds {
        let (parked, took, checked) = round(index, args, root, clock)?;
        progress(&format!(
            "restate round {} of {}: {parked} runs parked, {took} us from kill to done",
            index + 1,
            args.rounds
        ))?;
        parked_per_round.push(parked);
        per_round.push(took);
        steps_checked = steps_checked.saturating_add(checked);
    }
    let mut sorted = per_round.clone();
    Ok(Recovery {
        rounds: per_round.len(),
        resumed_runs: parked_per_round.iter().map(|count| u64::from(*count)).sum(),
        parked_runs_per_round: parked_per_round,
        kill_to_done_us: Summary::of(&mut sorted),
        per_round_us: per_round,
        steps_checked,
        duplicate_step_executions: 0,
    })
}

/// Every row `args.rows` names, measured on the same workload, as one table.
///
/// Each row works in its own directory under `root`. With `--recover` each row
/// is also killed and recovered.
///
/// # Errors
///
/// Any [`BenchError`] a row raises, or the one `progress` returns.
pub(crate) fn rows(
    args: &Args,
    root: &Path,
    clock: &Clock,
    progress: &mut dyn FnMut(&str) -> Result<(), BenchError>,
) -> Result<TableDoc, BenchError> {
    let recovering = args.mode == crate::args::Mode::Recover;
    let mut table = Vec::new();
    for row in &args.rows {
        let dir = root.join(row.name());
        std::fs::create_dir(&dir).map_err(|source| BenchError::Io {
            context: "making a row directory",
            source,
        })?;
        progress(&format!("measuring the {} row: {} runs", row.name(), args.runs))?;
        let mut doc = match row {
            Row::Lgwks => RowDoc::of_lgwks(&measure::flow(args, &dir, clock.clone())?),
            Row::Restate => flow(args, &dir, clock)?,
        };
        if recovering {
            let rounds = dir.join("recover");
            std::fs::create_dir(&rounds).map_err(|source| BenchError::Io {
                context: "making a recovery directory",
                source,
            })?;
            doc.recovery = Some(match row {
                Row::Lgwks => recover_lgwks(args, &rounds, clock.clone(), progress)?.recovery,
                Row::Restate => recover(args, &rounds, clock, progress)?,
            });
        }
        table.push(doc);
    }
    Ok(TableDoc {
        envelope: Envelope::new(&args.command, args.seed, args.runs)?,
        mode: "table",
        tenants: TENANTS.len(),
        workers: WORKERS,
        workload: Workload::of(Latencies::WORKLOAD),
        rows: table,
    })
}
