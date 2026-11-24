//! #1128 G4: the supervisor's two process ledgers drain.
//!
//! [`ORPHANS`] parks a child whose owner dropped it before it was reaped, and
//! on Linux `LIVE_LEADERS` holds every leader whose exit a driver still owns.
//! A ledger that only grows is a leak no assertion on a run's output sees: a
//! zombie per dropped child, or a pid the adoption sweep never collects.
//!
//! The oracle is the invariant, never the code's answer: after every seed,
//! with no further start, no leader is tracked and no orphan stays parked,
//! the ones a supervisor dropped mid-run included. Only the saboteur reaps by
//! hand: a live child parked by hand and a leader tracked by hand must read as
//! a full ledger, so an empty one below means something. Fools: the pulled
//! plug (a supervisor dropped with its children running), the clock-fool (a
//! deadline that kills mid-sleep), the double-clicker (seeds back to back on
//! one process).

#[cfg(target_os = "linux")]
use super::super::super::start::{LIVE_LEADERS, track_leader, untrack_leader};
use super::super::super::start::{ORPHANS, reap_orphans};
use super::super::super::supervisor::Supervisor;
use crate::rt::process::ProcessSpec;
use crate::rt::runtime::Runtime;
use lgwks_std::seeded::Seeded;
use std::time::{Duration, Instant};

/// What a sim reports when a precondition did not hold.
type SimResult = Result<(), Box<dyn std::error::Error>>;

/// How many seeds the family runs, each on a fresh runtime and supervisor.
const SEEDS: u64 = 24;

/// The shapes a supervised child is drawn from, by number.
const SHAPES: [&str; 4] = ["exit 0", "exit 3", "deadline kill", "spawned, left running"];

/// How long a SIGKILLed child may take to be torn down and reaped. Generous:
/// the kernel takes microseconds, and the bound only ends a red run.
const DRAIN_BOUND: Duration = Duration::from_secs(10);

/// A `sh -c` child running `script`.
fn shell(script: &str) -> ProcessSpec {
    let mut spec = ProcessSpec::new("sh");
    spec.arg("-c").arg(script);
    spec
}

/// How many children are parked in [`ORPHANS`] now.
fn parked() -> usize {
    ORPHANS.hold().len()
}

/// How many leaders are tracked now.
#[cfg(target_os = "linux")]
fn tracked() -> usize {
    LIVE_LEADERS.hold().len()
}

/// Whether [`ORPHANS`] empties within [`DRAIN_BOUND`], by observation alone:
/// `reap` runs only when `reap` is set, so the production path is what drains.
fn drains(reap: bool) -> bool {
    let began = Instant::now();
    while parked() != 0 {
        if reap {
            reap_orphans();
        }
        if began.elapsed() > DRAIN_BOUND {
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// The instrument, both ways: a live child parked by hand stays parked, and a
/// leader tracked by hand stays tracked, until they are released.
fn a_full_ledger_reads_full() -> SimResult {
    let mut sleeper = std::process::Command::new("sleep").arg("30").spawn()?;
    let pid = sleeper.id();
    if let Err(error) = sleeper.kill() {
        lgwks_std::trace::debug!(%error, pid, "ledgers: the planted child was not killed");
    }
    // Killed but not waited: a zombie the ledger must hold until it is reaped.
    ORPHANS.hold().push(sleeper);
    assert_eq!(parked(), 1, "a planted orphan reads as parked");
    #[cfg(target_os = "linux")]
    {
        track_leader(i32::MAX);
        assert_eq!(tracked(), 1, "a planted leader reads as tracked");
        untrack_leader(i32::MAX);
    }
    assert!(drains(true), "the planted orphan was never reaped");
    Ok(())
}

/// What one seed did, so the family can show its fools actually ran.
struct Seed {
    /// The supervisor was dropped with a spawned child still running.
    pulled_plug: bool,
}

/// One seed: a fresh supervisor runs one to four drawn children, then shuts
/// down or is dropped with its spawned children still running.
fn seed(seed: u64) -> Result<Seed, Box<dyn std::error::Error>> {
    let mut draw = Seeded::from_seed(seed);
    let count = draw.below(4)?.saturating_add(1);
    let shapes: Vec<usize> = (0..count)
        .map(|_| draw.index(SHAPES.len()))
        .collect::<Result<_, _>>()?;
    let shut_down = draw.below(2)? == 0;
    let left_running = shapes.contains(&3);
    let runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        for shape in shapes.iter().copied() {
            match shape {
                0 => {
                    let run = supervisor.run_process(&shell("exit 0")).await;
                    assert!(run.is_ok(), "seed {seed}: exit 0 ran: {run:?}");
                }
                1 => {
                    if let Err(error) = supervisor.run_process(&shell("exit 3")).await {
                        lgwks_std::trace::debug!(%error, seed, "ledgers: exit 3 reported");
                    }
                }
                2 => {
                    let mut spec = shell("exec sleep 30");
                    spec.deadline(Duration::from_millis(20));
                    if let Err(error) = supervisor.run_process(&spec).await {
                        lgwks_std::trace::debug!(%error, seed, "ledgers: the deadline ended it");
                    }
                }
                _ => {
                    supervisor.spawn_process(&shell("exec sleep 30")).await?;
                }
            }
        }
        if shut_down {
            let report = supervisor.shutdown().await;
            lgwks_std::trace::debug!(
                outcomes = report.outcomes().len(),
                seed,
                "ledgers: shut down"
            );
        }
        Ok::<_, std::io::Error>(())
    })?;
    Ok(Seed {
        pulled_plug: left_running && !shut_down,
    })
}

#[test]
fn sim_ledgers_drain_after_every_shape_of_run() -> SimResult {
    a_full_ledger_reads_full()?;
    let mut pulled_plugs = 0_u32;
    for number in 0..SEEDS {
        let ran = seed(number)?;
        pulled_plugs = pulled_plugs.saturating_add(u32::from(ran.pulled_plug));
        #[cfg(target_os = "linux")]
        assert_eq!(tracked(), 0, "seed {number}: a leader is still tracked");
        assert!(
            drains(false),
            "seed {number}: {} orphans stayed parked with no later start to reap them",
            parked()
        );
    }
    assert!(pulled_plugs > 0, "no seed dropped a supervisor mid-run");
    Ok(())
}
