//! A shell stopped by its own SIGSTOP, released by SIGCONT (#691).
//!
//! Every wait here is bounded. Source: run 44841657328, where the macOS shard
//! sent one SIGCONT to a stopped shell and its `Child::wait` blocked for 180 s
//! until the job cap cancelled it.

use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use lgwks_std::process::{
    child_has_exited_without_reaping, child_stopped_without_reaping, continue_process,
};

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// How long a shell gets to stop, or to exit once released.
const BOUND: Duration = Duration::from_secs(10);

/// A `/bin/sh` running `script`, with no stdio.
fn shell(script: &str) -> std::io::Result<Child> {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// The instant [`BOUND`] from now, or the child killed when the clock cannot hold it.
fn deadline(child: &mut Child) -> Result<Instant, Box<dyn std::error::Error>> {
    if let Some(at) = Instant::now().checked_add(BOUND) {
        return Ok(at);
    }
    child.kill()?;
    Err("the deadline overflowed the clock".into())
}

/// Release `child` with SIGCONT until it exits, then reap it, within [`BOUND`].
///
/// One CONT can be lost: on Darwin one that lands while the shell is still
/// stopping leaves it stopped, and a shell that stops again needs another. A
/// CONT to a running process changes nothing, so it is sent on every turn
/// until `waitid` reports the exit, and only then does `Child::wait` reap.
/// `None` is a shell still running at the bound, killed.
fn continue_until_exit(
    child: &mut Child,
) -> Result<Option<ExitStatus>, Box<dyn std::error::Error>> {
    let pid = i32::try_from(child.id())?;
    let deadline = deadline(child)?;
    loop {
        continue_process(pid)?;
        if child_has_exited_without_reaping(pid)? {
            return Ok(Some(child.wait()?));
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        std::thread::yield_now();
    }
}

/// A shell that stops itself stays stopped until it is continued.
///
/// The gate shell's contract in one process: `kill -STOP` stops the shell on
/// its first instruction, a waiter observes the stop without reaping, and
/// SIGCONT resumes exactly that shell to its own exit.
#[test]
fn a_self_stopping_shell_survives_until_continued() -> TestResult {
    let mut child = shell("kill -STOP $$; exit 42")?;
    let pid = i32::try_from(child.id())?;
    let deadline = deadline(&mut child)?;
    loop {
        if child_has_exited_without_reaping(pid)? {
            child.wait()?;
            return Err("a self-stopping shell must stop, not exit".into());
        }
        if child_stopped_without_reaping(pid)? {
            break;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            return Err("a self-stopping shell never stopped".into());
        }
        // Yield rather than park: the stop lands on the shell's first
        // instruction, so the wait is a reschedule, never a sleep.
        std::thread::yield_now();
    }
    let Some(status) = continue_until_exit(&mut child)? else {
        return Err("a continued shell never exited".into());
    };
    assert_eq!(
        status.code(),
        Some(42),
        "a continued shell runs its payload to its own exit: {status:?}"
    );
    Ok(())
}

/// A release that does not take is sent again until the shell exits.
///
/// A shell that stops itself twice is the lost release made deterministic:
/// one CONT leaves it stopped, so only a repeated one runs it to its exit.
#[test]
fn a_shell_stopped_twice_is_continued_to_its_exit() -> TestResult {
    let mut child = shell("kill -STOP $$; kill -STOP $$; exit 42")?;
    let Some(status) = continue_until_exit(&mut child)? else {
        return Err("a shell stopped twice never exited".into());
    };
    assert_eq!(
        status.code(),
        Some(42),
        "a shell stopped twice is continued to its own exit: {status:?}"
    );
    Ok(())
}
