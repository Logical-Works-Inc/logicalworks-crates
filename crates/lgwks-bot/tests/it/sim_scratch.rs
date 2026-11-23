//! Simulation family: no supervised child leaves a byte in temp (INV-BOT-TEMP-1).
//!
//! Source: 2026-11-23, the estate's build Mac. A fault lane whose children run
//! `git` and `cc` under a file-size limit left 110,965 `xcrun_db-*` files
//! (86 GiB) in the user temp dir, and test children killed by deadlines left
//! their temp trees beside them; the disk passed its 256 GiB cap twice in a
//! day. The Director: "THE WHOLE MOTHERFUCKING CLAIM OF USING CRATES IS A
//! RUNTIME FREE OF SHIT LIKE THIS."
//!
//! Every scenario runs a **real** `sh` through the real
//! [`Supervisor::run_process`]. The seed draws how the child ends (exits, fails,
//! is killed by its deadline, leaves a background writer) and how many files it
//! writes; the oracle is the filesystem after the run, never the crate's report.
//!
//! | Test | Property it pins |
//! |---|---|
//! | `sim_scratch_leaves_nothing` | each child's `TMPDIR` is its own directory under the user temp dir, and it and its process root are gone once the run returns, however the child ended |
//! | `sim_scratch_caller_temp_is_the_callers` | a spec that names `TMPDIR` keeps it, and what the child wrote there stays |
//! | `sim_scratch_sweeps_dead_roots_and_xcrun_leftovers` | a root whose owner lock is free is removed, a locked one stays, and only idle `xcrun_db-XXXXXXXX` files are swept |

#![cfg(all(
    unix,
    feature = "rt",
    feature = "time",
    feature = "sync",
    feature = "process"
))]

use crate::sim;

use std::error::Error;
use std::fs::{self, File};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lgwks_bot::rt::process::{ProcessRun, ProcessSpec};
use lgwks_bot::rt::runtime::Runtime;
use lgwks_bot::rt::supervise::Supervisor;

/// What a scenario reports when a precondition did not hold.
type TestResult = Result<(), Box<dyn Error>>;

/// The seeds the leak family sweeps, as one band.
const SEEDS: u64 = 48;

/// The retained-byte ceiling for the child's one printed line.
const CAPTURE: usize = 1 << 16;

/// The deadline a killed child runs into.
const KILL_AFTER: Duration = Duration::from_millis(150);

/// How a drawn child ends.
const ENDINGS: [&str; 4] = ["exits", "fails", "deadline", "background"];

/// The user temp dir every child directory must sit under.
fn user_temp() -> Result<PathBuf, Box<dyn Error>> {
    Ok(std::env::temp_dir().canonicalize()?)
}

/// 64 random bits as hex: a fixture name no concurrent test process repeats.
fn tag() -> Result<u64, Box<dyn Error>> {
    Ok(u64::from_be_bytes(lgwks_std::random::bytes::<8>()?))
}

/// A `sh -c script` spec with its stdout captured.
fn shell(script: &str) -> Result<ProcessSpec, Box<dyn Error>> {
    let mut spec = ProcessSpec::new("/bin/sh");
    spec.arg("-c").arg(script);
    spec.capture_stdout(NonZeroUsize::new(CAPTURE).ok_or("the capture ceiling is not zero")?);
    Ok(spec)
}

/// The script for one drawn child: print `$TMPDIR`, write `files` files and a
/// nested directory there, then end as `ending` says.
fn script(files: u64, ending: &str) -> String {
    let tail = match ending {
        "fails" => "exit 3",
        "deadline" => "while :; do :; done",
        "background" => {
            "( i=0; while [ $i -lt 50 ]; do : > \"$TMPDIR/late.$i\"; i=$((i+1)); done ) & exit 0"
        }
        _ => "exit 0",
    };
    format!(
        "printf '%s\\n' \"$TMPDIR\"; mkdir -p \"$TMPDIR/nested/deep\"; i=0; \
         while [ $i -lt {files} ]; do printf x > \"$TMPDIR/f.$i\"; i=$((i+1)); done; {tail}"
    )
}

/// The directory the child printed as its `TMPDIR`.
fn printed_dir(run: &ProcessRun) -> Result<PathBuf, Box<dyn Error>> {
    let line = std::str::from_utf8(run.stdout().bytes())?
        .lines()
        .next()
        .ok_or("the child printed no TMPDIR")?
        .to_owned();
    Ok(PathBuf::from(line))
}

/// One seed: run a drawn child and judge the filesystem it left.
fn leak_case(sim: &mut sim::Sim, runtime: &Runtime, temp: &Path) -> TestResult {
    let ending = *sim::nth(&ENDINGS, usize::try_from(sim.rng().below(4))?)?;
    let files = sim.rng().between(1, 40);
    let mut spec = shell(&script(u64::from(files), ending))?;
    if ending == "deadline" {
        spec.deadline(KILL_AFTER);
    }
    let mut supervisor = Supervisor::new(1);
    let run = runtime.block_on(supervisor.run_process(&spec))?;
    let dir = printed_dir(&run)?;
    assert!(
        dir.starts_with(temp) && dir != temp,
        "a child's TMPDIR is its own directory under the user temp dir: {} is not under {}",
        dir.display(),
        temp.display()
    );
    assert!(
        !dir.exists(),
        "the child's temp dir is gone once the run returns ({ending}): {}",
        dir.display()
    );
    // No other child of this test process is live, so the process root that
    // held the child's directory goes with its last child: a run leaves no
    // `lgwks-tmp-*` root, `.owner` file included (#1128 G2).
    let root = dir.parent().ok_or("a child's directory has a root")?;
    assert!(
        !root.exists(),
        "the process's temp root is gone with its last child ({ending}): {}",
        root.display()
    );
    assert_eq!(
        run.deadline_fired(),
        ending == "deadline",
        "only the drawn deadline fires"
    );
    sim.record(ending);
    Ok(())
}

/// Every drawn child's temp dir is private and gone after its run.
#[test]
fn sim_scratch_leaves_nothing() -> TestResult {
    let runtime = Runtime::new()?;
    let temp = user_temp()?;
    sim::assert_replays(sim::bands(SEEDS, 1).swap_remove(0), |sim| {
        leak_case(sim, &runtime, &temp)
    })
}

/// A spec that names `TMPDIR` keeps it, and the files stay the caller's.
#[test]
fn sim_scratch_caller_temp_is_the_callers() -> TestResult {
    let runtime = Runtime::new()?;
    let mine = user_temp()?.join(format!("lgwks-it-caller-temp-{}", tag()?));
    fs::create_dir_all(&mine)?;
    let mut spec = shell(&script(3, "exits"))?;
    spec.env("TMPDIR", &mine);
    let mut supervisor = Supervisor::new(1);
    let run = runtime.block_on(supervisor.run_process(&spec))?;
    assert_eq!(
        printed_dir(&run)?,
        mine,
        "the caller's TMPDIR is the child's"
    );
    assert!(
        mine.join("f.2").exists(),
        "what the child wrote there stays"
    );
    fs::remove_dir_all(&mine)?;
    Ok(())
}

/// A root left by a dead owner is removed by the next supervisor's first
/// spawn, a root whose owner still holds its lock stays, and only idle files
/// shaped like `xcrun`'s abandoned cache temps are swept.
#[test]
fn sim_scratch_sweeps_dead_roots_and_xcrun_leftovers() -> TestResult {
    let temp = user_temp()?;
    let tag = tag()?;
    let dead = temp.join(format!("lgwks-tmp-it-dead-{tag}"));
    let live = temp.join(format!("lgwks-tmp-it-live-{tag}"));
    // A fixed instant in 2001, far past any idle bound, so the test reads no clock.
    let long_ago = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(1_000_000_000))
        .ok_or("2001 is past the end of the clock")?;
    for root in [&dead, &live] {
        fs::create_dir_all(root.join("7"))?;
        fs::write(root.join("7/left"), b"x")?;
        // An owner that made its root long ago: a younger root is still being
        // opened, and the sweep leaves it alone.
        File::create(root.join(".owner"))?.set_modified(long_ago)?;
    }
    let held = File::open(live.join(".owner"))?;
    held.lock()?;
    let idle = temp.join(format!("xcrun_db-I{:07}", tag % 10_000_000));
    let fresh = temp.join(format!("xcrun_db-F{:07}", tag % 10_000_000));
    let other = temp.join(format!("xcrun_db-keep-{tag}"));
    for file in [&idle, &fresh, &other] {
        fs::write(file, b"cut")?;
    }
    File::options()
        .write(true)
        .open(&idle)?
        .set_modified(long_ago)?;
    File::options()
        .write(true)
        .open(&other)?
        .set_modified(long_ago)?;
    let runtime = Runtime::new()?;
    let mut supervisor = Supervisor::new(1);
    let run = runtime.block_on(supervisor.run_process(&shell("exit 0")?))?;
    assert_eq!(run.exit_code(), Some(0), "the probe child exits zero");
    assert!(!dead.exists(), "a dead owner's root is removed");
    assert!(live.join("7/left").exists(), "a live owner's root stays");
    assert!(!idle.exists(), "an idle xcrun_db-XXXXXXXX file is swept");
    assert!(fresh.exists(), "a file xcrun may still be writing stays");
    assert!(other.exists(), "a name xcrun never writes stays");
    drop(held);
    fs::remove_dir_all(&live)?;
    fs::remove_file(&fresh)?;
    fs::remove_file(&other)?;
    Ok(())
}
