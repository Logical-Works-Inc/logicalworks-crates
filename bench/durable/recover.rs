//! Recovery after `kill -9`: park runs in a child process, kill it while they
//! are mid-flight, reopen its run stores here and time the road to every run
//! done.
//!
//! The child parks `--runs` runs at their approval, tells the parent how many
//! parked on stdout, and waits to be killed, blocked on its stdin: the parent
//! holds the write end open, so the read ends only if the parent goes away
//! without killing it. The parent reads that one line (at
//! most 64 bytes of it), then sends the kill signal and starts its clock. The
//! clock covers the kill, the reaping, the reopen of both run stores, the
//! pending listing, every decision and every resume, and stops when the last run
//! is done. The checks that follow it (the step log) are outside the clock.
//!
//! Three things must hold or the round is an error, not a number: the reopened
//! stores list exactly the runs the child parked, every resumed run returns the
//! reference model's value, and no step body ran twice for one run.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lgwks_bot::clock::Clock;

use crate::args::Args;
use crate::error::{BenchError, ChildCause};
use crate::fleet::{
    Fleet, Parking, Rig, Started, TENANTS, WORKERS, grant_and_resume, parking_of, pool, pool_over,
    start, tenant_of,
};
use crate::report::{Durability, Envelope, RecoverDoc, Recovery, Workload};
use crate::stats::{Summary, micros};
use crate::workdir::Workdir;
use crate::world::{AppendLog, FAILURE_PERCENT, LOGGED_STEPS, Latencies, World};

/// The word that opens the child's one line on stdout.
const READY: &str = "ready";

/// The most bytes of the child's first line the parent reads.
const MARKER_BYTES: u64 = 64;

/// The file every step body appends a line to.
const EXEC_LOG: &str = "exec.log";

/// The file the child appends one line to for each run it parked.
const PARKED_LOG: &str = "parked.log";

/// The most pending approvals one listing asks for: the host's own ceiling per
/// tenant, and the recovery runs are capped below it.
const PENDING_LIMIT: usize = 10_000;

/// What one kill-and-resume round measured.
struct Round {
    /// Runs parked when the child was killed.
    parked: u32,
    /// Microseconds from the kill to the last run done.
    micros: u64,
    /// `sync_all` calls the reopened stores made.
    fsyncs: u64,
    /// Records the reopened stores staged.
    staged: u64,
    /// (run, step) pairs the step log was checked on.
    steps_checked: u64,
}

/// One parked run to decide and resume.
struct Job<'rig> {
    /// The tenant's rig.
    rig: &'rig Rig,
    /// The run's number.
    ordinal: u32,
    /// What a decision and a resume need.
    parking: Parking,
}

/// The child half: park every run, say how many parked, wait to be killed.
///
/// The wait is the event itself: a read of `input` that returns only when the
/// parent's end closes. The parent keeps that end open until it has killed the
/// child, so a kill ends this process mid-read and the read ending first means
/// the parent is gone.
///
/// # Errors
///
/// A [`BenchError`] for any run that ends in a way the workload does not name;
/// [`ChildCause::NothingParked`] when every run ended before it parked; and
/// [`ChildCause::NeverKilled`] when `input` closed before the kill came.
pub(crate) fn park_child(
    args: &Args,
    clock: Clock,
    mut out: impl Write,
    mut input: impl Read,
) -> Result<(), BenchError> {
    let given = args
        .work_dir
        .as_deref()
        .ok_or(BenchError::MissingValue { flag: "--work-dir" })?;
    let dir = Workdir::adopt(given)?;
    let steps = AppendLog::open(&dir.path().join(EXEC_LOG), "opening the step log")?;
    let parked_log = AppendLog::open(&dir.path().join(PARKED_LOG), "opening the parked log")?;
    let world = World::plan(
        args.seed,
        args.runs,
        FAILURE_PERCENT,
        Latencies::WORKLOAD,
        clock,
        Some(steps),
    )?;
    let fleet = Fleet::open(dir.path(), Arc::new(world))?;
    let flags = pool(args.runs, WORKERS, |ordinal| {
        match lgwks_bot::rt::runtime::block_on(start(fleet.rig(ordinal), ordinal))? {
            Started::Parked(parking) => {
                let line = format!(
                    "{} {ordinal} {}",
                    tenant_of(ordinal),
                    parking.run.id().to_hex()
                );
                parked_log.append(&line).map_err(|source| BenchError::Io {
                    context: "writing the parked log",
                    source,
                })?;
                Ok(1_u32)
            }
            Started::Finished(_ended) => Ok(0_u32),
        }
    })?;
    let parked: u32 = flags.iter().sum();
    if parked == 0 {
        lgwks_std::trace::debug!(runs = args.runs, "every run ended before it parked");
        return Err(BenchError::ChildFailed {
            cause: ChildCause::NothingParked,
        });
    }
    writeln!(out, "{READY} {parked}")
        .and_then(|()| out.flush())
        .map_err(|source| BenchError::Io {
            context: "announcing readiness",
            source,
        })?;
    io::copy(&mut input, &mut io::sink()).map_err(|source| BenchError::Io {
        context: "waiting on stdin to be killed",
        source,
    })?;
    drop(fleet);
    lgwks_std::trace::debug!("stdin closed before the parent's kill");
    Err(BenchError::ChildFailed {
        cause: ChildCause::NeverKilled,
    })
}

/// The count on the child's ready line.
fn parse_ready(line: &str) -> Result<u32, BenchError> {
    let text = line.trim_end();
    let bad = || BenchError::ChildFailed {
        cause: ChildCause::BadMarker {
            line: text.to_owned(),
        },
    };
    let count = text
        .strip_prefix(READY)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or_else(bad)?;
    count.parse::<u32>().map_err(|source| {
        lgwks_std::trace::debug!(%source, line = text, "the ready line's count is not a number");
        bad()
    })
}

/// Read the child's ready line, at most [`MARKER_BYTES`] of it.
fn await_ready(child: &mut Child) -> Result<u32, BenchError> {
    let stdout = child.stdout.take().ok_or_else(|| BenchError::Io {
        context: "taking the child's stdout",
        source: io::Error::other("the pipe was not made"),
    })?;
    let mut line = String::new();
    let read = BufReader::new(stdout.take(MARKER_BYTES))
        .read_line(&mut line)
        .map_err(|source| BenchError::Io {
            context: "reading the child's ready line",
            source,
        })?;
    if read == 0 {
        let status = child.wait().map_err(|source| BenchError::Io {
            context: "waiting for the child",
            source,
        })?;
        lgwks_std::trace::debug!(%status, "the child ended before it printed its ready line");
        return Err(BenchError::ChildFailed {
            cause: ChildCause::ExitedBeforeReady {
                status: status.to_string(),
            },
        });
    }
    parse_ready(&line)
}

/// Whether an exit is the kill this rig sent: signal 9 where processes end on
/// signals, else exit code 1, which `TerminateProcess` gives a killed child.
/// One rule for every platform, so each branch is tested on any host.
fn ended_by_kill(signals: bool, signal: Option<i32>, code: Option<i32>) -> bool {
    if signals {
        signal == Some(9)
    } else {
        code == Some(1)
    }
}

/// Whether the child ended on the kill signal.
#[cfg(unix)]
fn killed(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    ended_by_kill(true, status.signal(), status.code())
}

/// Whether the child ended the way a terminated process does on this platform.
#[cfg(not(unix))]
fn killed(status: ExitStatus) -> bool {
    ended_by_kill(false, None, status.code())
}

/// Send the kill signal and wait for the child to be gone.
fn kill_and_reap(child: &mut Child) -> Result<(), BenchError> {
    child.kill().map_err(|source| BenchError::Io {
        context: "killing the child",
        source,
    })?;
    let status = child.wait().map_err(|source| BenchError::Io {
        context: "waiting for the killed child",
        source,
    })?;
    if killed(status) {
        Ok(())
    } else {
        lgwks_std::trace::debug!(%status, "the child ended, but not by the kill signal");
        Err(BenchError::ChildFailed {
            cause: ChildCause::NotKilled {
                status: status.to_string(),
            },
        })
    }
}

/// The runs the child parked, from its log: (tenant slot, run id) to ordinal.
fn parse_parked(text: &str) -> Result<HashMap<(usize, String), u32>, BenchError> {
    let mut parked = HashMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let bad = || BenchError::MalformedRecord {
            file: PARKED_LOG,
            line: line.to_owned(),
        };
        let mut words = line.split_whitespace();
        let (Some(tenant), Some(ordinal), Some(run), None) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            lgwks_std::trace::debug!(line, file = PARKED_LOG, "a line does not have three words");
            return Err(bad());
        };
        let slot = TENANTS
            .iter()
            .position(|name| *name == tenant)
            .ok_or_else(bad)?;
        let ordinal: u32 = ordinal.parse().map_err(|source| {
            lgwks_std::trace::debug!(%source, line, file = PARKED_LOG, "a run number is not a number");
            bad()
        })?;
        if parked.insert((slot, run.to_owned()), ordinal).is_some() {
            lgwks_std::trace::debug!(line, file = PARKED_LOG, "a run is parked twice");
            return Err(bad());
        }
    }
    Ok(parked)
}

/// Check the step log: no step body ran twice, and each recovered run ran all
/// of its logged steps. Returns how many (run, step) pairs were checked.
pub(crate) fn check_steps(log: &str, recovered: &[(&'static str, u32)]) -> Result<u64, BenchError> {
    let mut seen: HashMap<(&str, u32, &str), u32> = HashMap::new();
    for line in log.lines().filter(|line| !line.trim().is_empty()) {
        let bad = || BenchError::MalformedRecord {
            file: EXEC_LOG,
            line: line.to_owned(),
        };
        let mut words = line.split_whitespace();
        let (Some(tenant), Some(ordinal), Some(step), None) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            lgwks_std::trace::debug!(line, file = EXEC_LOG, "a line does not have three words");
            return Err(bad());
        };
        let ordinal: u32 = ordinal.parse().map_err(|source| {
            lgwks_std::trace::debug!(%source, line, file = EXEC_LOG, "a run number is not a number");
            bad()
        })?;
        let count = seen.entry((tenant, ordinal, step)).or_insert(0);
        *count = count.saturating_add(1);
    }
    let duplicates = seen.values().filter(|count| **count > 1).count();
    if duplicates > 0 {
        lgwks_std::trace::debug!(duplicates, "a step body ran more than once");
        return Err(BenchError::StepRanTwice { duplicates });
    }
    let mut checked = 0_u64;
    for &(tenant, ordinal) in recovered {
        for step in LOGGED_STEPS {
            if !seen.contains_key(&(tenant, ordinal, step)) {
                lgwks_std::trace::debug!(tenant, ordinal, step, "a recovered run never ran a step");
                return Err(BenchError::StepMissing {
                    tenant: tenant.to_owned(),
                    ordinal,
                    step: step.to_owned(),
                });
            }
            checked += 1;
        }
    }
    Ok(checked)
}

/// Read what the child said it parked, and check the log agrees with the count
/// it announced.
fn read_parked(dir: &Path, announced: u32) -> Result<HashMap<(usize, String), u32>, BenchError> {
    let text = std::fs::read_to_string(dir.join(PARKED_LOG)).map_err(|source| BenchError::Io {
        context: "reading the parked log",
        source,
    })?;
    let parked = parse_parked(&text)?;
    if u32::try_from(parked.len()) != Ok(announced) {
        lgwks_std::trace::debug!(
            announced,
            logged = parked.len(),
            "the parked log disagrees with the child"
        );
        return Err(BenchError::MalformedRecord {
            file: PARKED_LOG,
            line: format!(
                "the child announced {announced} runs, the log holds {}",
                parked.len()
            ),
        });
    }
    Ok(parked)
}

/// The pending approvals of the reopened stores, each matched to the run the
/// child parked.
fn jobs_of<'rig>(
    fleet: &'rig Fleet,
    parked: &HashMap<(usize, String), u32>,
) -> Result<Vec<Job<'rig>>, BenchError> {
    let mut jobs = Vec::new();
    for (slot, rig) in fleet.rigs().iter().enumerate() {
        let pending = rig
            .host
            .pending(PENDING_LIMIT)
            .map_err(|source| BenchError::Flow {
                stage: "listing the pending approvals",
                source,
            })?;
        for item in &pending {
            let run = item.run().id().to_hex();
            let ordinal = parked.get(&(slot, run.clone())).copied().ok_or_else(|| {
                BenchError::MalformedRecord {
                    file: PARKED_LOG,
                    line: format!("the store lists run {run}, which the child never parked"),
                }
            })?;
            jobs.push(Job {
                rig,
                ordinal,
                parking: parking_of(item),
            });
        }
    }
    let expected = u32::try_from(parked.len()).map_err(|source| {
        lgwks_std::trace::debug!(%source, parked = parked.len(), "more runs parked than a job number holds");
        BenchError::OutsideJobList { index: u32::MAX }
    })?;
    let found = u32::try_from(jobs.len()).map_err(|source| {
        lgwks_std::trace::debug!(%source, jobs = jobs.len(), "more jobs than a job number holds");
        BenchError::OutsideJobList { index: u32::MAX }
    })?;
    if found < expected {
        lgwks_std::trace::debug!(
            expected,
            found,
            "the reopened stores list fewer runs than parked"
        );
        return Err(BenchError::LostRuns { expected, found });
    }
    Ok(jobs)
}

/// One round: park, kill, reopen, resume, check.
fn round(index: u32, args: &Args, root: &Path, clock: Clock) -> Result<Round, BenchError> {
    let dir = root.join(format!("round-{index}"));
    std::fs::create_dir(&dir).map_err(|source| BenchError::Io {
        context: "making the round directory",
        source,
    })?;
    let seed = args.seed.wrapping_add(u64::from(index));
    let steps = AppendLog::open(&dir.join(EXEC_LOG), "opening the step log")?;
    let world = Arc::new(World::plan(
        seed,
        args.runs,
        FAILURE_PERCENT,
        Latencies::WORKLOAD,
        clock,
        Some(steps),
    )?);
    let exe = std::env::current_exe().map_err(|source| BenchError::Io {
        context: "finding this executable",
        source,
    })?;
    let mut child = Command::new(exe)
        .args(["--park-child", "--runs"])
        .arg(args.runs.to_string())
        .arg("--seed")
        .arg(seed.to_string())
        .arg("--work-dir")
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|source| BenchError::Io {
            context: "starting the child",
            source,
        })?;
    let listing = await_ready(&mut child)
        .and_then(|announced| read_parked(&dir, announced).map(|parked| (announced, parked)));
    // The child dies now whatever the listing says, so no child outlives a round.
    let clock = Instant::now();
    let reaped = kill_and_reap(&mut child);
    let (announced, parked) = listing?;
    reaped?;
    let fleet = Fleet::open(&dir, world)?;
    let jobs = jobs_of(&fleet, &parked)?;
    pool_over(&jobs, WORKERS, |job| {
        lgwks_bot::rt::runtime::block_on(grant_and_resume(
            job.rig,
            job.ordinal,
            &job.parking,
            Duration::ZERO,
        ))
    })?;
    let took = micros(clock.elapsed());
    let log = std::fs::read_to_string(dir.join(EXEC_LOG)).map_err(|source| BenchError::Io {
        context: "reading the step log",
        source,
    })?;
    let recovered: Vec<(&'static str, u32)> = jobs
        .iter()
        .map(|job| (tenant_of(job.ordinal), job.ordinal))
        .collect();
    let steps_checked = check_steps(&log, &recovered)?;
    let (fsyncs, staged) = fleet.flush_counts();
    Ok(Round {
        parked: announced,
        micros: took,
        fsyncs,
        staged,
        steps_checked,
    })
}

/// Take `args.rounds` rounds under `root` and report them.
///
/// The durability counts are the reopened stores' side: the syncs and records of
/// the recovery itself, summed over the rounds.
///
/// # Errors
///
/// The first [`BenchError`] a round raises, or the one `progress` returns.
pub(crate) fn recover(
    args: &Args,
    root: &Path,
    clock: Clock,
    progress: &mut dyn FnMut(&str) -> Result<(), BenchError>,
) -> Result<RecoverDoc, BenchError> {
    let mut rounds = Vec::new();
    for index in 0..args.rounds {
        let done = round(index, args, root, clock.clone())?;
        progress(&format!(
            "round {} of {}: {} runs parked, {} us from kill to done",
            index + 1,
            args.rounds,
            done.parked,
            done.micros
        ))?;
        rounds.push(done);
    }
    let per_round_us: Vec<u64> = rounds.iter().map(|done| done.micros).collect();
    let mut sorted = per_round_us.clone();
    let kill_to_done_us = Summary::of(&mut sorted);
    let sum = |pick: fn(&Round) -> u64| {
        rounds
            .iter()
            .fold(0_u64, |total, done| total.saturating_add(pick(done)))
    };
    let resumed_runs = sum(|done| u64::from(done.parked));
    Ok(RecoverDoc {
        envelope: Envelope::new(&args.command, args.seed, args.runs)?,
        mode: "recover",
        tenants: TENANTS.len(),
        durability: Durability::new(sum(|done| done.fsyncs), sum(|done| done.staged)),
        workload: Workload::of(Latencies::WORKLOAD),
        recovery: Recovery {
            rounds: rounds.len(),
            parked_runs_per_round: rounds.iter().map(|done| done.parked).collect(),
            kill_to_done_us,
            per_round_us,
            resumed_runs,
            steps_checked: sum(|done| done.steps_checked),
            duplicate_step_executions: 0,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lgwks_std::seeded::Seeded;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// A clean step log for runs `0..runs`: every logged step once per run.
    fn clean_log(runs: u32) -> Vec<String> {
        let mut lines = Vec::new();
        for ordinal in 0..runs {
            for step in LOGGED_STEPS {
                lines.push(format!("{} {ordinal} {step}", tenant_of(ordinal)));
            }
        }
        lines
    }

    /// Every run of `0..runs`, as the checker takes them.
    fn all_runs(runs: u32) -> Vec<(&'static str, u32)> {
        (0..runs)
            .map(|ordinal| (tenant_of(ordinal), ordinal))
            .collect()
    }

    /// A clean log passes and counts its pairs; over many seeds a planted fault is
    /// always found, and a duplicate and a missing step are told apart.
    #[test]
    fn sim_the_step_checker_finds_every_planted_fault() -> TestResult {
        let mut stream = Seeded::from_seed(7);
        let mut draw = |bound| stream.below(bound).map_err(|error| error.to_string());
        for _ in 0..200 {
            let runs = 1 + u32::try_from(draw(12)?).map_err(|error| error.to_string())?;
            let clean = clean_log(runs);
            let recovered = all_runs(runs);
            let want = u64::from(runs) * 9;
            assert_eq!(
                check_steps(&clean.join("\n"), &recovered).map_err(|e| e.line())?,
                want
            );
            let at = usize::try_from(draw(clean.len() as u64)?).map_err(|e| e.to_string())?;
            let mut duplicated = clean.clone();
            duplicated.insert(at, clean[at].clone());
            assert!(matches!(
                check_steps(&duplicated.join("\n"), &recovered),
                Err(BenchError::StepRanTwice { duplicates: 1 })
            ));
            let dropped: Vec<String> = clean
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != at)
                .map(|(_, row)| row.clone())
                .collect();
            assert!(matches!(
                check_steps(&dropped.join("\n"), &recovered),
                Err(BenchError::StepMissing { .. })
            ));
        }
        Ok(())
    }

    /// A line that does not read back is a typed refusal that names the file.
    #[test]
    fn sim_hostile_log_lines_are_refused_typed() -> TestResult {
        let hostile = [
            "alpha",
            "alpha 4",
            "alpha four s1-intake",
            "alpha 4 s1-intake extra",
            "alpha -1 s1-intake",
            "alpha 99999999999 s1-intake",
        ];
        for line in hostile {
            assert!(
                matches!(
                    check_steps(line, &[]),
                    Err(BenchError::MalformedRecord { file: EXEC_LOG, .. })
                ),
                "{line}"
            );
        }
        let parked = [
            "gamma 4 00000000000000000000000000000001",
            "alpha x 00000000000000000000000000000001",
            "alpha 4",
        ];
        for line in parked {
            assert!(
                matches!(
                    parse_parked(line),
                    Err(BenchError::MalformedRecord {
                        file: PARKED_LOG,
                        ..
                    })
                ),
                "{line}"
            );
        }
        let twice = "alpha 4 aa\nalpha 6 aa";
        assert!(parse_parked(twice).is_err());
        Ok(())
    }

    /// A parked log reads back to the runs it names, tenant by tenant.
    #[test]
    fn the_parked_log_reads_back_by_tenant_and_run() -> TestResult {
        let text = "alpha 4 aa\nbeta 7 bb\n\n";
        let parked = parse_parked(text).map_err(|e| e.line())?;
        assert_eq!(parked.get(&(0, "aa".to_owned())), Some(&4));
        assert_eq!(parked.get(&(1, "bb".to_owned())), Some(&7));
        assert_eq!(parked.get(&(1, "aa".to_owned())), None);
        Ok(())
    }

    /// The ready line is `ready N` and nothing else.
    #[test]
    fn the_ready_line_parses_or_refuses_typed() {
        assert!(matches!(parse_ready("ready 12\n"), Ok(12)));
        for line in [
            "",
            "ready",
            "ready x",
            "ready 1 2",
            "reddy 3",
            "ready -4",
            " ready 5",
        ] {
            assert!(
                matches!(
                    parse_ready(line),
                    Err(BenchError::ChildFailed {
                        cause: ChildCause::BadMarker { .. }
                    })
                ),
                "{line:?}"
            );
        }
    }

    /// Only the kill the rig sent counts: signal 9 on a signal platform, exit
    /// code 1 elsewhere. A clean exit, another signal or another code does not.
    #[test]
    fn only_the_kill_the_rig_sent_counts_as_killed() {
        let cases = [
            (true, Some(9), None, true),
            (true, Some(15), None, false),
            (true, None, Some(0), false),
            (true, None, Some(1), false),
            (false, None, Some(1), true),
            (false, None, Some(0), false),
            (false, Some(9), Some(137), false),
            (false, None, None, false),
        ];
        for (signals, signal, code, want) in cases {
            assert_eq!(
                ended_by_kill(signals, signal, code),
                want,
                "{signals} {signal:?} {code:?}"
            );
        }
    }
}
