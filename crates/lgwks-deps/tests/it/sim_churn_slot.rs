//! The `churn-slot` wrapper, run as nextest runs it: a real process in front of a
//! real program, with the machine's slots as real lock files (#272).
//!
//! What a user of the wrapper sees is three things. The program runs with the
//! arguments and environment it was given and the wrapper's status is the
//! program's; no more than `LWC_CHURN_SLOTS` programs run at once, however many
//! wrappers are started; and a machine where the slots cannot be made still runs
//! the program, because a test that does not run is a test that silently passes.
//! Each family is swept over seeded draws of the slot count, the number of waiting
//! wrappers and the exit status, twice per seed with the receipt compared, so one
//! seed is one replayable run. The holders block on a pipe the test closes, never
//! on a timer: no assertion here waits for time to pass.

use crate::replay::{Failure, refuse, replay_sweep};
use crate::sim::{Rng, Trace};

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// The wrapper under test.
const WRAPPER: &str = env!("CARGO_BIN_EXE_churn-slot");

/// Seeds per family: each is two runs of up to seven processes.
const SEEDS: u64 = 16;

/// A directory of this run's own, made empty: the family, the seed and a random
/// draw name it, so two runs on one machine never share one.
fn scratch(family: &str, seed: u64) -> Result<PathBuf, Failure> {
    let distinguisher = u64::from_le_bytes(lgwks_std::random::bytes::<8>()?);
    let dir = std::env::temp_dir().join(format!(
        "churn-slot-it-{family}-{seed:x}-{distinguisher:016x}"
    ));
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            lgwks_std::trace::debug!(%error, "churn-slot sim: an old scratch dir stayed");
            return Err(error.into());
        }
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Removes a directory this run made. A refusal is logged and the run goes
/// on: under the census it then fails this test by name anyway.
fn clear(dir: &Path) {
    if let Err(error) = fs::remove_dir_all(dir) {
        lgwks_std::trace::debug!(%error, dir = %dir.display(), "clear: a scratch dir was not removed");
    }
}

/// The wrapper in front of `program`, with `slots` slots under `dir`.
fn wrapper(dir: &Path, slots: usize) -> Command {
    let mut command = Command::new(WRAPPER);
    command
        .env("LWC_CHURN_DIR", dir)
        .env("LWC_CHURN_SLOTS", slots.to_string());
    command
}

/// A wrapped program that announces itself and then waits for its input to close.
struct Holder {
    /// The wrapper process.
    child: Child,
    /// What the program reads; closing it lets the program exit.
    stdin: Option<ChildStdin>,
    /// What the program prints.
    stdout: BufReader<ChildStdout>,
}

impl Holder {
    /// Starts a wrapper in front of a program that prints `ready` and reads until
    /// its input closes.
    fn start(dir: &Path, slots: usize) -> Result<Self, Failure> {
        let mut child = wrapper(dir, slots)
            .args(["/bin/sh", "-c", "echo ready; cat > /dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("the holder has a stdout pipe")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// Blocks until the program has printed `ready`, which it does only once the
    /// wrapper has given it a slot.
    fn wait_ready(&mut self) -> Result<(), Failure> {
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        if line.trim() != "ready" {
            return refuse(format!("a holder printed {line:?} instead of ready"));
        }
        Ok(())
    }

    /// Lets the program exit and waits for the wrapper, returning its status.
    fn release(mut self) -> Result<i32, Failure> {
        // Closing the pipe is what lets the program exit.
        self.stdin = None;
        let status = self.child.wait()?;
        status
            .code()
            .ok_or_else(|| "the wrapper ended without an exit status".into())
    }
}

/// Whether every slot file under `dir` is locked by someone else.
fn every_slot_is_held(dir: &Path, slots: usize) -> Result<bool, Failure> {
    for index in 0..slots {
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(format!("slot-{index}")))?;
        match file.try_lock() {
            Ok(()) => return Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(cause)) => {
                lgwks_std::trace::debug!(error = ?cause, "every_slot_is_held: a slot file would not lock");
                return Err(cause.into());
            }
        }
    }
    Ok(true)
}

/// A seeded slot count (1 to 3) and number of waiting wrappers (1 to 3).
fn shape(rng: &mut Rng) -> Result<(usize, usize), Failure> {
    let slots = usize::try_from(rng.between(1, 3))?;
    let waiters = usize::try_from(rng.between(1, 3))?;
    Ok((slots, waiters))
}

#[test]
fn the_wrapper_never_runs_more_programs_than_there_are_slots() -> Result<(), Failure> {
    replay_sweep("slot-bound", SEEDS, |rng, trace: &mut Trace| {
        let (slots, waiters) = shape(rng)?;
        let dir = scratch("bound", rng.next_u64())?;
        let mut holders = Vec::new();
        for _ in 0..slots {
            let mut holder = Holder::start(&dir, slots)?;
            holder.wait_ready()?;
            holders.push(holder);
        }
        if !every_slot_is_held(&dir, slots)? {
            return refuse(format!(
                "{slots} programs run and a slot of {slots} is not locked"
            ));
        }
        let mut waiting = Vec::new();
        for _ in 0..waiters {
            waiting.push(Holder::start(&dir, slots)?);
        }
        let mut statuses = 0_u64;
        for holder in holders {
            statuses = statuses.saturating_add(u64::from(holder.release()? == 0));
        }
        // A waiter that finds every slot held waits on the one its random start
        // names, so two waiters can queue on one slot while another is free, and
        // the one that wins is not the first started. Each waiter is therefore
        // read and released on its own thread; reading them in start order would
        // wait on a program the first reader's slot holds back.
        let finished = std::thread::scope(|scope| {
            let mut threads = Vec::new();
            for mut holder in waiting {
                threads.push(scope.spawn(move || -> Result<i32, String> {
                    holder.wait_ready().map_err(|error| error.to_string())?;
                    holder.release().map_err(|error| error.to_string())
                }));
            }
            let mut outcomes = Vec::new();
            for thread in threads {
                outcomes.push(thread.join());
            }
            outcomes
        });
        for outcome in finished {
            match outcome {
                Ok(Ok(status)) => {
                    statuses = statuses.saturating_add(u64::from(status == 0));
                }
                Ok(Err(cause)) => return refuse(format!("a waiter failed: {cause}")),
                Err(_) => return refuse("a waiter's thread panicked"),
            }
        }
        trace.record_number("slots", u64::try_from(slots)?);
        trace.record_number("waiters", u64::try_from(waiters)?);
        trace.record_number("clean", statuses);
        clear(&dir);
        let expected = u64::try_from(slots.saturating_add(waiters))?;
        if statuses != expected {
            return refuse(format!("{statuses} of {expected} wrappers exited cleanly"));
        }
        Ok(())
    })
}

#[test]
fn the_program_gets_its_arguments_its_environment_and_reports_its_own_status() -> Result<(), Failure>
{
    replay_sweep("passthrough", SEEDS, |rng, trace: &mut Trace| {
        let code = rng.between(0, 255);
        let marker = rng.next_u64();
        let dir = scratch("passthrough", marker)?;
        let output = wrapper(&dir, 2)
            .env("CHURN_PROBE", marker.to_string())
            .args([
                "/bin/sh",
                "-c",
                "printf '%s %s' \"$CHURN_PROBE\" \"$1\"; exit \"$2\"",
                "probe",
            ])
            .arg("an argument")
            .arg(code.to_string())
            .output()?;
        clear(&dir);
        let printed = String::from_utf8(output.stdout)?;
        let expected = format!("{marker} an argument");
        if printed != expected {
            return refuse(format!("the program printed {printed:?}, not {expected:?}"));
        }
        let status = output
            .status
            .code()
            .ok_or("the wrapper has an exit status")?;
        if u32::try_from(status)? != code {
            return refuse(format!("the wrapper exited {status}, the program {code}"));
        }
        trace.record_number("code", u64::from(code));
        trace.record_number("marker", marker);
        Ok(())
    })
}

#[test]
fn slots_that_cannot_be_made_still_run_the_program_and_say_so() -> Result<(), Failure> {
    replay_sweep("uncapped", SEEDS, |rng, trace: &mut Trace| {
        let code = rng.between(0, 200);
        let dir = scratch("uncapped", rng.next_u64())?;
        let blocker = dir.join("not-a-directory");
        File::create(&blocker)?;
        let output = wrapper(&blocker, 2)
            .args(["/bin/sh", "-c", "exit \"$1\"", "probe"])
            .arg(code.to_string())
            .output()?;
        clear(&dir);
        let said = String::from_utf8(output.stderr)?;
        if !said.contains("running uncapped") {
            return refuse(format!("the wrapper did not say it ran uncapped: {said:?}"));
        }
        let status = output
            .status
            .code()
            .ok_or("the wrapper has an exit status")?;
        if u32::try_from(status)? != code {
            return refuse(format!("the wrapper exited {status}, the program {code}"));
        }
        trace.record_number("code", u64::from(code));
        Ok(())
    })
}

#[test]
fn a_program_that_cannot_start_and_a_call_with_none_are_refused_by_name() -> Result<(), Failure> {
    replay_sweep("refusals", SEEDS, |rng, trace: &mut Trace| {
        let dir = scratch("refusals", rng.next_u64())?;
        let missing = wrapper(&dir, 1).arg(dir.join("no-such-program")).output()?;
        let none = wrapper(&dir, 1).output()?;
        clear(&dir);
        if missing.status.code() != Some(127) {
            return refuse(format!(
                "a program that cannot start exited {:?}, not 127",
                missing.status.code()
            ));
        }
        if none.status.code() != Some(2) {
            return refuse(format!(
                "a call with no program exited {:?}, not 2",
                none.status.code()
            ));
        }
        trace.record_number("missing", 127);
        trace.record_number("none", 2);
        Ok(())
    })
}

/// What a program the wrapper starts sees as `TMPDIR` when its caller gave
/// `tmpdir`, the wrapper's exit status, and what it said on standard error.
fn seen_tmpdir(
    dir: &Path,
    tmpdir: &str,
    choice: Option<&str>,
) -> Result<(String, Option<i32>, String), Failure> {
    let mut command = wrapper(dir, 2);
    command
        .env("TMPDIR", tmpdir)
        .args(["/bin/sh", "-c", "printf '%s' \"$TMPDIR\""]);
    match choice {
        Some(said) => command.env("LWC_CHURN_TMP", said),
        None => command.env_remove("LWC_CHURN_TMP"),
    };
    let output = command.output()?;
    Ok((
        String::from_utf8(output.stdout)?,
        output.status.code(),
        String::from_utf8(output.stderr)?,
    ))
}

/// Whether `seen` is a census dir of the wrapper's own directly under `base`:
/// `<base>/lwc-test-<16 hex>/` (ADR 0078).
fn own_dir_under(seen: &str, base: &str) -> bool {
    seen.strip_prefix(base.trim_end_matches('/'))
        .and_then(|rest| rest.strip_prefix("/lwc-test-"))
        .and_then(|rest| rest.strip_suffix('/'))
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Whether the wrapper refused to run a test whose temp dir does not exist.
fn refused_unjudged(seen: &str, code: Option<i32>, said: &str) -> bool {
    seen.is_empty() && code == Some(125) && said.contains("no census dir under")
}

/// Catches a wrapper that leaves a test on the physical disk's fsync queue, or
/// that moves a directory its caller chose (a CI job's own RAM disk). Every
/// program sees a census dir of its own under the base it was given, and a
/// base that does not exist is refused by name rather than run unjudged.
#[test]
fn only_a_default_temporary_directory_is_moved_to_the_shared_volume() -> Result<(), Failure> {
    replay_sweep("shared-temp", SEEDS, |rng, trace: &mut Trace| {
        let marker = rng.next_u64();
        let dir = scratch("shared-temp", marker)?;
        let default = format!("/var/folders/{marker:x}/T/");
        let chosen = dir.join(format!("chosen-{marker:x}"));
        fs::create_dir(&chosen)?;
        let chosen = chosen.to_str().ok_or("a scratch path is UTF-8")?.to_owned();
        let (moved, moved_code, said) = seen_tmpdir(&dir, &default, None)?;
        let (kept, _, _) = seen_tmpdir(&dir, &chosen, None)?;
        let chosen_left = fs::read_dir(&chosen)?.count();
        let (inherited, inherited_code, inherited_said) =
            seen_tmpdir(&dir, &default, Some("inherit"))?;
        clear(&dir);
        let shared = own_dir_under(&moved, "/Volumes/lgwks-test-tmp/tmp/");
        let declined =
            said.contains("keeping TMPDIR") && refused_unjudged(&moved, moved_code, &said);
        if cfg!(target_os = "macos") && !shared && !declined {
            return refuse(format!(
                "a default TMPDIR became {moved:?} and the wrapper said {said:?}"
            ));
        }
        if !cfg!(target_os = "macos") && !refused_unjudged(&moved, moved_code, &said) {
            return refuse(format!(
                "off macOS a missing default TMPDIR became {moved:?}: {said:?}"
            ));
        }
        if !own_dir_under(&kept, &chosen) || chosen_left != 0 {
            return refuse(format!(
                "a chosen TMPDIR became {kept:?}, and {chosen_left} entries stayed in it"
            ));
        }
        if !refused_unjudged(&inherited, inherited_code, &inherited_said) {
            return refuse(format!(
                "an inherited missing TMPDIR became {inherited:?}: {inherited_said:?}"
            ));
        }
        trace.record_number("shared", u64::from(shared));
        Ok(())
    })
}

/// The census's verdict for one drawn program: the exit status the wrapper
/// owes, from what the program was told to leave and how it ends.
fn owed_status(code: u8, entries: u64, orphan: bool) -> i32 {
    match code {
        0 if entries > 0 || orphan => 1,
        _ => i32::from(code),
    }
}

/// Catches a wrapper that lets a test pass while it leaves files in its temp
/// dir or, on Linux, a live process behind it (ADR 0078, #1128 G2). Source:
/// 2026-11-23, 110,965 leaked temp files on the build Mac that no test saw.
/// Fools: the monkey writes nested trees, the saboteur forks a background
/// process and exits, and a failing program keeps its own status.
#[test]
fn the_census_fails_a_program_that_leaves_residue() -> Result<(), Failure> {
    replay_sweep("residue", SEEDS, |rng, trace: &mut Trace| {
        let dir = scratch("residue", rng.next_u64())?;
        let base = dir.join("base");
        fs::create_dir(&base)?;
        let trees = rng.between(0, 6);
        let code = if rng.between(0, 3) == 0 { 7_u8 } else { 0 };
        // Only Linux has the subreaper the process census needs.
        let orphan = cfg!(target_os = "linux") && rng.between(0, 2) == 0;
        let background = if orphan {
            "sleep 600 </dev/null >/dev/null 2>&1 &"
        } else {
            ""
        };
        let script = format!(
            "printf '%s' \"$TMPDIR\"; i=0; while [ $i -lt {trees} ]; do \
             mkdir -p \"$TMPDIR/t$i/deep\"; : > \"$TMPDIR/t$i/deep/f\"; i=$((i+1)); done; \
             {background} exit {code}"
        );
        let output = wrapper(&dir, 2)
            .env("TMPDIR", &base)
            .env("LWC_CHURN_TMP", "inherit")
            .args(["/bin/sh", "-c", &script])
            .output()?;
        let seen = String::from_utf8(output.stdout)?;
        let said = String::from_utf8(output.stderr)?;
        let left_in_base = fs::read_dir(&base)?.count();
        clear(&dir);
        if !own_dir_under(&seen, base.to_str().ok_or("a scratch path is UTF-8")?) {
            return refuse(format!(
                "the program's TMPDIR {seen:?} is not its own dir under {base:?}"
            ));
        }
        if left_in_base != 0 {
            return refuse(format!(
                "the wrapper left {left_in_base} entries in the base temp dir"
            ));
        }
        // Each tree is three entries: t<i>, t<i>/deep and t<i>/deep/f.
        let entries = trees.saturating_mul(3);
        let status = output
            .status
            .code()
            .ok_or("the wrapper has an exit status")?;
        let owed = owed_status(code, u64::from(entries), orphan);
        if status != owed {
            return refuse(format!(
                "the wrapper exited {status}, owed {owed} (code {code}, {entries} entries, orphan {orphan}): {said:?}"
            ));
        }
        if entries > 0 && !said.contains(&format!("the test left {entries} entries")) {
            return refuse(format!(
                "the verdict does not count {entries} entries: {said:?}"
            ));
        }
        if orphan && !said.contains("live processes, now killed") {
            return refuse(format!(
                "the verdict does not name the live process: {said:?}"
            ));
        }
        trace.record_number("entries", u64::from(entries));
        trace.record_number("orphan", u64::from(orphan));
        trace.record_number("status", u64::try_from(status)?);
        Ok(())
    })
}

#[test]
fn a_program_that_has_exited_leaves_no_slot_held() -> Result<(), Failure> {
    replay_sweep("released", SEEDS, |rng, trace: &mut Trace| {
        let slots = usize::try_from(rng.between(1, 3))?;
        let dir = scratch("released", rng.next_u64())?;
        let mut holder = Holder::start(&dir, slots)?;
        holder.wait_ready()?;
        let held = every_slot_is_held(&dir, 1)?;
        if let Some(mut input) = holder.stdin.take() {
            input.write_all(b"done\n")?;
        }
        let status = holder.release()?;
        let free = !every_slot_is_held(&dir, slots)?;
        clear(&dir);
        if status != 0 || !free || (slots == 1 && !held) {
            return refuse(format!(
                "a released holder left status {status}, free = {free}, held while running = {held}"
            ));
        }
        trace.record_number("slots", u64::try_from(slots)?);
        Ok(())
    })
}
