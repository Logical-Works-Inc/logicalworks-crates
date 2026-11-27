//! Seeded fire test of `lgwks_std::process::Command`, the one process start
//! (#1164, ADR 0092): no child outlives its owner, by any end.
//!
//! The tests of two consumers started servers with a raw `std` command, and a
//! SIGKILL of the test left them holding a 24 GB Mac for hours (#1160). Each
//! case here starts a group whose leader puts a `sleep` in the background, the
//! shape of a server started by a script, and ends it by one of the ways a
//! caller ends a child: it runs to its end through `output`, `status`, `wait` or
//! `try_wait`, the handle is dropped or killed, or the owner itself is
//! SIGKILLed. Every end must leave neither the leader nor the background member
//! running. The processes are real: a group kill and a parent's death are not
//! observable through a stub. Only the seed's draws are folded into the trace.

#![cfg(all(unix, feature = "process"))]

use std::error::Error;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use lgwks_std::process::{Child, Command, running_processes};
use lgwks_std::seeded::Seeded;

use crate::seeded_bytes::{below, poll_within};
use crate::seeded_sweep::{SWEEP_SEEDS, fold, initial_trace};

/// What every test here returns.
type TestResult = Result<(), Box<dyn Error>>;

/// The variable that puts this binary in the owner role, naming its directory.
const OWNER_VAR: &str = "LGWKS_COMMAND_OWNER";

/// Cases per seed.
const CASES: usize = 8;

/// How many process-table probes a stop may take: a count, so the bound is the
/// same on every run, and a process that never stops fails in seconds.
const TABLE_BUDGET: u32 = 1 << 14;

/// How many probes a file another process writes may take to arrive.
const FILE_BUDGET: u32 = 1 << 22;

/// A leader that backgrounds a member, says its pid, and ends at once.
const ENDS: &str = "sleep 300 & echo $!";

/// A leader that backgrounds a member, says its pid, and runs on.
const RUNS: &str = "sleep 300 & echo $!; exec sleep 300";

/// The ways a case ends its child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// `Command::output` on a leader that ends.
    Output,
    /// `Command::status` on a leader that ends.
    Status,
    /// `Child::wait` on a leader that ends.
    Wait,
    /// `Child::try_wait` until a leader that ends has ended.
    TryWait,
    /// The handle of a running leader is dropped.
    Drop,
    /// The handle of a running leader is killed, then waited.
    Kill,
    /// The owner holding a running leader is SIGKILLed.
    OwnerKilled,
}

/// Every end, in draw order.
const ENDS_DRAWN: [End; 7] = [
    End::Output,
    End::Status,
    End::Wait,
    End::TryWait,
    End::Drop,
    End::Kill,
    End::OwnerKilled,
];

/// Whether every pid in `pids` stops running within the budget.
fn all_stop(pids: &[i32]) -> Result<bool, Box<dyn Error>> {
    let stopped = poll_within(TABLE_BUDGET, || {
        Ok(running_processes(pids)?.is_empty().then_some(()))
    })?;
    Ok(stopped.is_some())
}

/// A leader running `script` under `sh`, its output piped.
fn start(script: &str) -> Result<Child, Box<dyn Error>> {
    Ok(Command::new("/bin/sh")
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?)
}

/// The member pid the leader of `child` printed first.
fn member_of(child: &mut Child) -> Result<i32, Box<dyn Error>> {
    let out = child.take_stdout().ok_or("the leader's output is piped")?;
    let mut line = String::new();
    std::io::BufReader::new(out).read_line(&mut line)?;
    Ok(line.trim().parse()?)
}

/// The pid `text` names on its first line.
fn first_pid(text: &[u8]) -> Result<i32, Box<dyn Error>> {
    let said = String::from_utf8_lossy(text);
    Ok(said
        .lines()
        .next()
        .ok_or("the leader said its member")?
        .trim()
        .parse()?)
}

/// A case directory, removed when the case ends.
struct CaseDir(PathBuf);

impl Drop for CaseDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::debug!(%error, "a command case directory could not be removed");
        }
    }
}

/// The owner role: start a running leader, write its pids, wait to be killed.
fn act_as_owner(dir: &Path) -> TestResult {
    let mut child = start(RUNS)?;
    let member = member_of(&mut child)?;
    let partial = dir.join("pids.partial");
    std::fs::write(&partial, format!("{} {member}", child.id()))?;
    std::fs::rename(&partial, dir.join("pids"))?;
    let mut sink = String::new();
    while std::io::stdin().read_line(&mut sink)? > 0 {}
    drop(child);
    Ok(())
}

/// Start an owner, wait for its pids, SIGKILL it, and answer the pids.
fn kill_an_owner(tag: &str) -> Result<Vec<i32>, Box<dyn Error>> {
    let dir =
        CaseDir(std::env::temp_dir().join(format!("lgwks-command-{}-{tag}", std::process::id())));
    std::fs::create_dir_all(&dir.0)?;
    let mut owner = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "sim_command::sim_command_owns_every_child_by_every_end",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(OWNER_VAR, &dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let marker = dir.0.join("pids");
    let said = poll_within(FILE_BUDGET, || match std::fs::read_to_string(&marker) {
        Ok(text) => Ok(Some(Some(text))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(owner.try_wait()?.map(|_| None))
        }
        Err(error) => Err(error.into()),
    })?
    .flatten()
    .ok_or("the owner never started its leader")?;
    // The owner's input stays open until it is dead, so it never takes the
    // normal exit that lets its child go.
    let keep_open = owner.take_stdin();
    owner.kill()?;
    owner.wait()?;
    drop(keep_open);
    said.split_whitespace()
        .map(|pid| Ok(pid.parse::<i32>()?))
        .collect()
}

/// Run one case to `end` and answer the pids that must then be stopped.
fn one_case(end: End, tag: &str) -> Result<Vec<i32>, Box<dyn Error>> {
    Ok(match end {
        End::Output => {
            let said = Command::new("/bin/sh").args(["-c", ENDS]).output()?;
            assert!(said.status.success(), "{tag}: the leader ran");
            vec![first_pid(&said.stdout)?]
        }
        End::Status => {
            let dir = CaseDir(
                std::env::temp_dir()
                    .join(format!("lgwks-command-status-{}-{tag}", std::process::id())),
            );
            std::fs::create_dir_all(&dir.0)?;
            let file = dir.0.join("member");
            let script = format!("sleep 300 & echo $! > {}", file.display());
            let status = Command::new("/bin/sh").args(["-c", &script]).status()?;
            assert!(status.success(), "{tag}: the leader ran");
            vec![std::fs::read_to_string(&file)?.trim().parse()?]
        }
        End::Wait => {
            let mut child = start(ENDS)?;
            let member = member_of(&mut child)?;
            child.wait()?;
            vec![member]
        }
        End::TryWait => {
            let mut child = start(ENDS)?;
            let member = member_of(&mut child)?;
            poll_within(FILE_BUDGET, || Ok(child.try_wait()?))?
                .ok_or("an ended leader never read as ended")?;
            vec![member]
        }
        End::Drop => {
            let mut child = start(RUNS)?;
            let member = member_of(&mut child)?;
            let leader = i32::try_from(child.id())?;
            drop(child);
            vec![leader, member]
        }
        End::Kill => {
            let mut child = start(RUNS)?;
            let member = member_of(&mut child)?;
            let leader = i32::try_from(child.id())?;
            child.kill()?;
            child.wait()?;
            vec![leader, member]
        }
        End::OwnerKilled => kill_an_owner(tag)?,
    })
}

/// Every end, drawn by seed: the leader and the member it put in the background
/// both stop, whether the leader ended, the handle went, or the owner died.
#[test]
fn sim_command_owns_every_child_by_every_end() -> TestResult {
    if let Some(dir) = std::env::var_os(OWNER_VAR) {
        return act_as_owner(Path::new(&dir));
    }
    for seed in SWEEP_SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let mut trace = initial_trace();
        for case in 0..CASES {
            let end = *ENDS_DRAWN
                .get(below(&mut rng, ENDS_DRAWN.len()))
                .ok_or("an end past the list")?;
            let tag = format!("{seed:x}-{case}");
            let pids = one_case(end, &tag)?;
            let running = running_processes(&pids)?;
            assert!(
                all_stop(&pids)?,
                "{tag}: {end:?} left {pids:?} running ({running:?} at first look)"
            );
            fold(&mut trace, u64::try_from(case)?);
        }
        assert_ne!(trace, initial_trace(), "seed {seed:#x} drew its cases");
    }
    // Every end is driven on every run, whatever the seeds drew.
    for (index, end) in ENDS_DRAWN.iter().enumerate() {
        let pids = one_case(*end, &format!("each-{index}"))?;
        assert!(all_stop(&pids)?, "{end:?} left {pids:?} running");
    }
    Ok(())
}
