//! Seeded fire test of the lifeline: an owner SIGKILLed with holds open (#1160).
//!
//! A supervisor killed with SIGKILL runs no code, so nothing it owns is released
//! by it. The lifeline is the release that does not need the owner: a process
//! the owner feeds hold and release lines, which on the owner's death kills
//! every group still held and runs every release program still held, and then
//! ends.
//!
//! Each case starts an owner, which is this test binary run again in the owner
//! role. The seed draws how many groups it holds, which it releases itself
//! before the kill, and whether its lifeline is SIGKILLed while it lives, so the
//! respawn and replay path is driven too. The parent then SIGKILLs the owner and
//! asserts what a user of the rails is promised: every held group and its
//! member stop, the held release programs ran, the released one did not, and
//! the lifeline itself ends. The processes are real: a group kill and a parent
//! death are not observable through a stub. Only the seed's draws are folded
//! into the trace, so a seed replays the same trace.

#![cfg(all(unix, feature = "process"))]

use std::error::Error;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use lgwks_std::process::{
    LifelineHold, hold_group, hold_run, kill_process, kill_process_group, lifeline_pid,
    running_processes,
};
use lgwks_std::seeded::Seeded;

use crate::group_leader::spawn_group_leader;
use crate::seeded_bytes::{below, poll_within};
use crate::seeded_sweep::{SWEEP_SEEDS, fold, fold_usize, initial_trace};

/// What every test here returns: a fixture that cannot be built fails the test
/// with its cause instead of panicking.
type TestResult = Result<(), Box<dyn Error>>;

/// The variable that puts this binary in the owner role, holding its plan.
const OWNER_VAR: &str = "LGWKS_LIFELINE_OWNER";

/// The directory an owner writes its rosters and markers in.
const DIR_VAR: &str = "LGWKS_LIFELINE_DIR";

/// The prefix of the one line an owner prints once its holds are open.
const READY: &str = "lifeline-owner: ready";

/// Most groups one owner holds.
const MAX_GROUPS: usize = 3;

/// Owners per seed.
const CASES: usize = 2;

/// How many probes a fact may take to arrive: a count, so the bound is the same
/// on every run.
const PROBE_BUDGET: u32 = 1 << 22;

/// How many process-table probes a stop may take. Off Linux each one reads the
/// whole table, near a millisecond on a loaded Mac, so a process that never
/// stops fails the test with its message in seconds, not at the run's timeout.
const TABLE_BUDGET: u32 = 1 << 14;

/// What the seed drew for one owner.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Plan {
    /// Per group: whether the owner releases it before it is killed.
    released: Vec<bool>,
    /// Whether the owner SIGKILLs its lifeline after the first hold.
    kill_lifeline: bool,
}

impl Plan {
    /// Draw a plan.
    fn draw(rng: &mut Seeded) -> Self {
        let groups = below(rng, MAX_GROUPS).saturating_add(1);
        let released = (0..groups).map(|_| below(rng, 3) == 0).collect();
        Self {
            released,
            kill_lifeline: below(rng, 2) == 0,
        }
    }

    /// The plan as the owner reads it from its environment.
    fn render(&self) -> String {
        let groups: String = self
            .released
            .iter()
            .map(|&released| if released { 'r' } else { 'h' })
            .collect();
        format!("{groups}:{}", u8::from(self.kill_lifeline))
    }

    /// The plan an owner was given.
    fn parse(text: &str) -> Result<Self, Box<dyn Error>> {
        let (groups, kill) = text.split_once(':').ok_or("a plan has two parts")?;
        Ok(Self {
            released: groups.chars().map(|mark| mark == 'r').collect(),
            kill_lifeline: kill == "1",
        })
    }
}

/// A group the owner started: `sh` leading, one background `sleep` as member.
/// Returns the leader and its pids, leader first.
fn start_group(dir: &Path, index: usize) -> Result<(Child, Vec<i32>), Box<dyn Error>> {
    let roster = dir.join(format!("group-{index}.pid"));
    let ready = dir.join(format!("group-{index}.ready"));
    let script = format!(
        "sleep 300 & echo $! > {}; : > {}; exec sleep 300",
        roster.display(),
        ready.display()
    );
    let leader = spawn_group_leader(&script)?;
    let root = i32::try_from(leader.id())?;
    poll_within(PROBE_BUDGET, || Ok(ready.exists().then_some(())))?
        .ok_or("a group leader never wrote its roster")?;
    let member: i32 = std::fs::read_to_string(&roster)?.trim().parse()?;
    Ok((leader, vec![root, member]))
}

/// Comma-joined pids.
fn join(pids: &[i32]) -> String {
    pids.iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// SIGKILL this owner's lifeline and wait until it has stopped running.
///
/// The lifeline is this owner's own child, so once killed it stays a zombie
/// until reaped, and a pid probe may still find it. `lifeline_pid` reaps it
/// (`try_wait`) and answers `None` once it is gone, so the wait ends on the
/// exit itself.
fn kill_own_lifeline() -> TestResult {
    let first = lifeline_pid().ok_or("a hold starts a lifeline")?;
    kill_process(first)?;
    poll_within(PROBE_BUDGET, || Ok(lifeline_pid().is_none().then_some(())))?
        .ok_or("the owner's lifeline outlived its SIGKILL")?;
    Ok(())
}

/// One line of the owner's progress on its stderr, which the parent keeps in
/// the case directory and shows when the owner never gets ready.
fn step(what: &str) -> TestResult {
    let mut err = std::io::stderr().lock();
    writeln!(err, "lifeline-owner: {what}")?;
    err.flush()?;
    Ok(())
}

/// The owner role: open the holds the plan names, say so, then wait to die.
fn act_as_owner(plan: &Plan, dir: &Path) -> TestResult {
    // A lifeline that dies while the owner lives is named on stderr, which the
    // parent shows with the verdict.
    lgwks_std::trace::install_default("lifeline-owner")?;
    let mkdir = Path::new("/bin/mkdir");
    let mut holds: Vec<LifelineHold> = vec![hold_run(mkdir, &dir.join("kept"))?];
    drop(hold_run(mkdir, &dir.join("dropped"))?);
    let mut held = Vec::new();
    let mut released = Vec::new();
    let mut leaders = Vec::new();
    step("run holds written")?;
    for (index, &release) in plan.released.iter().enumerate() {
        let (leader, pids) = start_group(dir, index)?;
        step(&format!(
            "group {index} started: {pids:?}, lifeline {:?}",
            lifeline_pid()
        ))?;
        let root = *pids.first().ok_or("a group has a leader")?;
        let hold = hold_group(root)?;
        step(&format!(
            "group {index} held on lifeline {:?}",
            lifeline_pid()
        ))?;
        if index == 0 && plan.kill_lifeline {
            kill_own_lifeline()?;
            step("own lifeline killed and reaped")?;
        }
        if release {
            kill_process_group(root)?;
            drop(hold);
            released.extend(pids);
        } else {
            holds.push(hold);
            held.extend(pids);
        }
        leaders.push(leader);
    }
    // One more hold after any lifeline kill, so a respawn has run and replayed
    // every hold before the parent strikes.
    holds.push(hold_run(mkdir, &dir.join("late"))?);
    step("late hold written")?;
    let line = lifeline_pid().ok_or("an owner with holds has a lifeline")?;
    // Written whole, then renamed, so the parent never reads half a line.
    let partial = dir.join("ready.partial");
    std::fs::write(
        &partial,
        format!(
            "{READY} lifeline={line} held={} released={}",
            join(&held),
            join(&released)
        ),
    )?;
    std::fs::rename(&partial, dir.join("ready"))?;
    // Wait for the parent's SIGKILL; its death closes stdin, which ends us too.
    let mut sink = String::new();
    while std::io::stdin().read_line(&mut sink)? > 0 {}
    drop(holds);
    drop(leaders);
    Ok(())
}

/// The pids after `key` on the ready line.
fn field(line: &str, key: &str) -> Result<Vec<i32>, Box<dyn Error>> {
    let value = line
        .split_whitespace()
        .find_map(|word| word.strip_prefix(key))
        .ok_or("the ready line names every field")?;
    value
        .split(',')
        .filter(|pid| !pid.is_empty())
        .map(|pid| Ok(pid.parse::<i32>()?))
        .collect()
}

/// A case directory, removed with everything in it when the case ends.
struct CaseDir(PathBuf);

impl Drop for CaseDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::debug!(%error, "a lifeline case directory could not be removed");
        }
    }
}

/// Whether every pid in `pids` stops running within the budget.
fn all_stop(pids: &[i32]) -> Result<bool, Box<dyn Error>> {
    let stopped = poll_within(TABLE_BUDGET, || {
        Ok(running_processes(pids)?.is_empty().then_some(()))
    })?;
    Ok(stopped.is_some())
}

/// Run one owner under `plan`, SIGKILL it, and check what was released.
fn one_case(plan: &Plan, tag: &str) -> TestResult {
    let dir =
        CaseDir(std::env::temp_dir().join(format!("lgwks-lifeline-{}-{tag}", std::process::id())));
    std::fs::create_dir_all(&dir.0)?;
    let said = dir.0.join("owner.stderr");
    let mut owner = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "sim_lifeline::sim_lifeline_releases_every_hold_of_a_killed_owner",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(OWNER_VAR, plan.render())
        .env(DIR_VAR, &dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&said)?)
        .spawn()?;
    // A file, not a pipe: a pipe some stray child still holds would block the
    // read past every bound. The wait ends on the file, or on the owner's exit.
    let marker = dir.0.join("ready");
    let ready = poll_within(PROBE_BUDGET, || match std::fs::read_to_string(&marker) {
        Ok(line) => Ok(Some(Some(line))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(owner.try_wait()?.map(|_| None))
        }
        Err(error) => Err(error.into()),
    })?
    .flatten();
    // `Child::wait` closes the child's stdin before it waits, and on a loaded
    // host the owner reads that end of file before the SIGKILL lands, takes
    // its normal exit and lets every hold go. The pipe stays open until the
    // owner is reaped, so the only way out is the kill.
    let keep_open = owner.stdin.take();
    owner.kill()?;
    let status = owner.wait()?;
    drop(keep_open);
    let steps = std::fs::read_to_string(&said)?;
    let ready = ready
        .ok_or_else(|| format!("{tag}: the owner never opened its holds ({status}): {steps}"))?;
    let held = field(&ready, "held=")?;
    let line = field(&ready, "lifeline=")?;
    // The lifeline ends last, after every release, so it is waited on first;
    // then every fact is read once and judged together.
    let line_stopped = all_stop(&line)?;
    let held_stopped = all_stop(&held)?;
    let (kept, late) = (dir.0.join("kept").is_dir(), dir.0.join("late").is_dir());
    let dropped = dir.0.join("dropped").exists();
    let entries = std::fs::read_dir(&dir.0)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    let running = running_processes(&[held.as_slice(), line.as_slice()].concat())?;
    assert!(
        line_stopped && held_stopped && kept && late && !dropped,
        "{tag}: the dead owner's holds were not all released: lifeline stopped {line_stopped}, \
         held groups stopped {held_stopped}, kept {kept}, late {late}, dropped ran {dropped}, \
         still running {running:?}, {plan:?}, {ready}, case dir holds {entries:?}\n{steps}"
    );
    Ok(())
}

/// Pieces a fat-fingered or hostile caller hands a hold: plain parts the line
/// carries, and bytes a shell reads as code or a line reads as a split.
const PIECES: [&str; 16] = [
    "disk", "a.b", "_+-", "9", "/", "", " ", "\n", ";", "$(id)", "`", "'", "\"", ",", "é", "*",
];

/// The model of ADR 0087 decision 6: a value the lifeline's line and `eval`
/// carry as is is not empty and has only letters, digits and `_ . / + -`.
fn carried(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./+-".contains(&byte))
}

/// A drawn value of one to three pieces.
fn drawn(rng: &mut Seeded) -> Result<String, Box<dyn Error>> {
    let mut value = String::new();
    for _ in 0..=below(rng, 3) {
        value.push_str(
            PIECES
                .get(below(rng, PIECES.len()))
                .ok_or("a piece past the list")?,
        );
    }
    Ok(value)
}

/// A weird-bytes caller: every hold of a drawn program, argument or device is
/// taken exactly when the model says the line can carry it, and refused as
/// `InvalidInput` otherwise, so no byte ever reaches the lifeline's `eval` as
/// code. A program always lies under a directory that does not exist and a
/// device past any a host has, so an accepted hold releases nothing even if
/// this test is killed holding it.
#[test]
fn sim_lifeline_refuses_every_value_its_line_cannot_carry() -> TestResult {
    if std::env::var_os(OWNER_VAR).is_some() {
        return Ok(());
    }
    for seed in SWEEP_SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let mut trace = initial_trace();
        for case in 0..64_usize {
            let tail = drawn(&mut rng)?;
            let absolute = below(&mut rng, 4) != 0;
            let program = format!(
                "{}nonexistent-lgwks-sim/{tail}",
                if absolute { "/" } else { "" }
            );
            let argument = drawn(&mut rng)?;
            let want = absolute && carried(&program) && carried(&argument);
            match hold_run(Path::new(&program), Path::new(&argument)) {
                Ok(hold) => {
                    drop(hold);
                    assert!(
                        want,
                        "seed {seed:#x} case {case}: {program:?} {argument:?} was taken"
                    );
                }
                Err(error) => {
                    assert!(
                        !want,
                        "seed {seed:#x} case {case}: {program:?} {argument:?}: {error}"
                    );
                    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
                }
            }
            // Past any disk a host has, so a kill while held detaches nothing.
            let device = format!("/dev/disk99999{}", drawn(&mut rng)?);
            let digits = device
                .strip_prefix("/dev/disk")
                .ok_or("a device has the prefix")?;
            let disk = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
            match lgwks_std::process::hold_device(&device) {
                Ok(hold) => {
                    drop(hold);
                    assert!(disk, "seed {seed:#x} case {case}: {device:?} was taken");
                }
                Err(error) => assert!(!disk, "seed {seed:#x} case {case}: {device:?}: {error}"),
            }
            fold(&mut trace, u64::from(want));
            fold(&mut trace, u64::from(disk));
        }
        assert_ne!(trace, initial_trace(), "seed {seed:#x} drew its cases");
    }
    Ok(())
}

/// The variable that puts this binary in the stream-owner role.
const STREAMS_VAR: &str = "LGWKS_LIFELINE_STREAMS";

/// The prefix of the stream owner's one line: its lifeline's pid.
const STREAMS_READY: &str = "lifeline-streams: lifeline=";

/// A lifeline that holds its owner's output pipes: an owner killed with a hold
/// open leaves its lifeline running the release, and the parent that reads the
/// owner's output to its end must not wait for that release. On 2026-11-26 a
/// lifeline on an inherited stderr held nextest's capture pipe past the test's
/// exit, which nextest reports as a leak (#1160). The release here is a
/// `sleep`, so a lifeline that held a stream would end before the read did.
#[test]
fn sim_lifeline_holds_none_of_its_owner_streams() -> TestResult {
    if std::env::var_os(STREAMS_VAR).is_some() {
        let hold = hold_run(Path::new("/bin/sleep"), Path::new("20"))?;
        let line = lifeline_pid().ok_or("a hold starts a lifeline")?;
        let mut out = std::io::stdout().lock();
        writeln!(out, "{STREAMS_READY}{line}")?;
        out.flush()?;
        // The owner dies with the hold open. A thread can run on for a moment
        // after the SIGKILL it sent itself, and a drop there lets the hold go,
        // so it waits on a message nobody sends while it owns the sender. The
        // channel holds no message, so a bound of one is never reached.
        let (sender, never) = std::sync::mpsc::sync_channel::<()>(1);
        kill_process(i32::try_from(std::process::id())?)?;
        let woke = never.recv();
        drop((sender, hold));
        woke?;
        return Ok(());
    }
    let owner = Command::new(std::env::current_exe()?)
        .args([
            "sim_lifeline::sim_lifeline_holds_none_of_its_owner_streams",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(STREAMS_VAR, "1")
        .stdin(Stdio::null())
        .output()?;
    let said = String::from_utf8_lossy(&owner.stdout);
    // The harness prints the test's name on the same line, before it.
    let line: i32 = said
        .split(STREAMS_READY)
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or_else(|| {
            format!(
                "the stream owner named no lifeline ({}): {said}",
                owner.status
            )
        })?
        .parse()?;
    let running = running_processes(&[line])?;
    // The lifeline leads its own group with its `sleep`, so one group kill
    // ends the release before the verdict.
    let killed = kill_process_group(line);
    assert!(
        running.contains(&line),
        "the owner's output ended only after its lifeline did: the lifeline held an owner stream"
    );
    killed?;
    assert!(
        all_stop(&[line])?,
        "the stream owner's lifeline outlived its group kill"
    );
    Ok(())
}

#[test]
fn sim_lifeline_releases_every_hold_of_a_killed_owner() -> TestResult {
    if let Ok(plan) = std::env::var(OWNER_VAR) {
        let dir = std::env::var_os(DIR_VAR).ok_or("an owner is given its directory")?;
        return act_as_owner(&Plan::parse(&plan)?, Path::new(&dir));
    }
    for seed in SWEEP_SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let mut replay = Seeded::from_seed(seed);
        let mut trace = initial_trace();
        for case in 0..CASES {
            let plan = Plan::draw(&mut rng);
            assert_eq!(
                plan,
                Plan::draw(&mut replay),
                "seed {seed:#x} case {case}: the plan replays"
            );
            assert_eq!(
                Plan::parse(&plan.render())?,
                plan,
                "seed {seed:#x} case {case}: the owner reads the plan it was given"
            );
            one_case(&plan, &format!("{seed:x}-{case}"))?;
            fold_usize(&mut trace, plan.released.len());
            fold(&mut trace, u64::from(plan.kill_lifeline));
        }
        assert_ne!(trace, initial_trace(), "seed {seed:#x} drew its cases");
    }
    Ok(())
}
