//! A started [`Child`], owned: its group ends with its leader, with its drop,
//! or with this process (#1164, ADR 0092).

use std::io;
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Output};

/// Kani proofs of the kill rounds for every small group.
#[cfg(all(any(kani, test), unix))]
mod proof;
/// The round bound, at the production value and each small one.
#[cfg(all(test, unix))]
mod sim_rounds;
/// The kill rounds driven against a seeded model of a group.
#[cfg(all(test, unix))]
mod tests;

/// A started child, owned: its group ends with its leader, with its drop, or
/// with this process.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// let child = lgwks_std::process::Command::new("sleep").arg("60").spawn()?;
/// drop(child); // the group is killed and reaped here
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Child {
    /// The child's input, when it was piped and not yet taken.
    stdin: Option<ChildStdin>,
    /// The child's output, when it was piped and not yet taken.
    stdout: Option<ChildStdout>,
    /// The child's error stream, when it was piped and not yet taken.
    stderr: Option<ChildStderr>,
    /// The leader.
    inner: std::process::Child,
    /// The group the leader leads (its pid).
    pgid: i32,
    /// The group's hold on the lifeline, let go once the group is gone.
    #[cfg(unix)]
    hold: Option<Hold>,
    /// The leader's status, once reaped.
    ended: Option<ExitStatus>,
}

/// The lifeline hold type of a target; off Unix there is none.
#[cfg(unix)]
type Hold = crate::process::LifelineHold;
/// See the Unix [`Hold`].
#[cfg(not(unix))]
type Hold = ();

impl Child {
    /// The child `inner`, leading `pgid`, held by `hold`.
    pub(super) fn owned(mut inner: std::process::Child, pgid: i32, hold: Option<Hold>) -> Self {
        #[cfg(not(unix))]
        let _no_lifeline = hold;
        Self {
            stdin: inner.stdin.take(),
            stdout: inner.stdout.take(),
            stderr: inner.stderr.take(),
            inner,
            pgid,
            #[cfg(unix)]
            hold,
            ended: None,
        }
    }

    /// The leader's pid, which is its group's id on Unix.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let child = lgwks_std::process::Command::new("sleep").arg("1").spawn()?;
    /// assert!(child.id() > 0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn id(&self) -> u32 {
        self.inner.id()
    }

    /// The child's piped input, taken: [`Child::wait`] closes it if it is
    /// still here.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// use std::io::Write as _;
    /// let mut child = lgwks_std::process::Command::new("cat")
    ///     .stdin(std::process::Stdio::piped())
    ///     .spawn()?;
    /// if let Some(mut input) = child.take_stdin() {
    ///     input.write_all(b"hi")?;
    /// }
    /// child.wait()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    /// The child's piped output, taken.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let mut child = lgwks_std::process::Command::new("echo")
    ///     .stdout(std::process::Stdio::piped())
    ///     .spawn()?;
    /// assert!(child.take_stdout().is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    /// Send the whole group SIGKILL. A group already gone is not an error.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let mut child = lgwks_std::process::Command::new("sleep").arg("60").spawn()?;
    /// child.kill()?;
    /// let _status = child.wait()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The kill's error for a group that is still there.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.ended.is_some() {
            return Ok(());
        }
        end_group(self.pgid)
    }

    /// Wait for the leader to end, kill what is left of its group, reap the
    /// leader and let the hold go. The input is closed first, as `std` does.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let mut child = lgwks_std::process::Command::new("true").spawn()?;
    /// assert!(child.wait()?.success());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The wait's error.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.ended {
            return Ok(status);
        }
        self.stdin = None;
        #[cfg(unix)]
        wait_exit_keeping(self.pgid)?;
        self.finish()
    }

    /// The leader's status when it has ended, and `None` while it runs. An
    /// ended leader's group is killed and the leader reaped, as [`Child::wait`]
    /// does.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let mut child = lgwks_std::process::Command::new("sleep").arg("60").spawn()?;
    /// assert!(child.try_wait()?.is_none());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The probe's or the reap's error.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.ended {
            return Ok(Some(status));
        }
        #[cfg(unix)]
        let exited = crate::process::child_has_exited_without_reaping(self.pgid)?;
        #[cfg(not(unix))]
        let exited = matches!(self.inner.try_wait()?, Some(_));
        if exited {
            self.finish().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Run the child to its end, reading both piped outputs while it runs.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let child = lgwks_std::process::Command::new("echo")
    ///     .arg("hi")
    ///     .stdout(std::process::Stdio::piped())
    ///     .spawn()?;
    /// assert_eq!(child.wait_with_output()?.stdout, b"hi\n");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// A read's or the wait's error.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        self.stdin = None;
        let (out, err) = (self.stdout.take(), self.stderr.take());
        let (status, stdout, stderr) = std::thread::scope(|scope| {
            let stdout = scope.spawn(move || read_all(out));
            let stderr = scope.spawn(move || read_all(err));
            let status = self.wait();
            (status, joined(stdout.join()), joined(stderr.join()))
        });
        Ok(Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    }

    /// The leader has ended: kill what is left of its group while the leader's
    /// unreaped pid still pins the group id, then reap it and let the hold go.
    fn finish(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        if let Err(error) = end_group(self.pgid) {
            trace_error(&error, "a finished child's group refused its kill");
        }
        let status = self.inner.wait()?;
        self.ended = Some(status);
        // The group is gone, so its hold is let go without a release.
        #[cfg(unix)]
        {
            self.hold = None;
        }
        Ok(status)
    }
}

impl Drop for Child {
    /// A child dropped before its leader was reaped: the group is killed and
    /// the leader reaped, so a dropped handle never leaves a process running.
    fn drop(&mut self) {
        if self.ended.is_some() {
            return;
        }
        if let Err(error) = end_group(self.pgid) {
            trace_error(&error, "a dropped child's group refused its kill");
        }
        #[cfg(not(unix))]
        if let Err(error) = self.inner.kill() {
            trace_error(&error, "a dropped child refused its kill");
        }
        if let Err(error) = self.inner.wait() {
            trace_error(&error, "a dropped child could not be reaped");
        }
    }
}

/// SIGKILL group `pgid`; a group already gone is not an error.
#[cfg(not(target_os = "macos"))]
fn end_group(pgid: i32) -> io::Result<()> {
    match crate::process::kill_process_group(pgid) {
        Err(error) if is_gone(&error) => Ok(()),
        other => other,
    }
}

/// Kill rounds a group may take on macOS before it is called unstoppable: a
/// group that adds a member every round is a fork bomb, not a leak.
#[cfg(any(target_os = "macos", all(any(test, kani), unix)))]
const GROUP_ROUNDS: usize = 64;

/// What ending a group asks of the host: a seam, so the kill rounds run
/// against a seeded model as well as the kernel.
#[cfg(any(target_os = "macos", all(any(test, kani), unix)))]
trait GroupHost {
    /// `killpg(pgid, SIGKILL)`.
    fn kill_group(&mut self, pgid: i32) -> io::Result<()>;
    /// A listing of group `pgid`, owned, so the kills it drives can borrow the
    /// host: a `Vec` from the kernel, a fixed array in the Kani proof, which
    /// then models no heap.
    type Members: IntoIterator<Item = (i32, bool)>;
    /// Each member of group `pgid`, and whether it is a zombie.
    fn members(&mut self, pgid: i32) -> io::Result<Self::Members>;
    /// `kill(pid, SIGKILL)`.
    fn kill_pid(&mut self, pid: i32) -> io::Result<()>;
}

/// The Darwin kernel behind [`GroupHost`].
#[cfg(target_os = "macos")]
struct Darwin;

#[cfg(target_os = "macos")]
impl GroupHost for Darwin {
    fn kill_group(&mut self, pgid: i32) -> io::Result<()> {
        crate::process::kill_process_group(pgid)
    }

    type Members = Vec<(i32, bool)>;

    fn members(&mut self, pgid: i32) -> io::Result<Self::Members> {
        let rows = crate::process::darwin::group(pgid)?;
        Ok(rows.into_iter().map(|row| (row.pid, row.zombie)).collect())
    }

    fn kill_pid(&mut self, pid: i32) -> io::Result<()> {
        crate::process::kill_process(pid)
    }
}

/// SIGKILL group `pgid`; a group already gone is not an error.
#[cfg(target_os = "macos")]
fn end_group(pgid: i32) -> io::Result<()> {
    end_group_on(&mut Darwin, pgid, GROUP_ROUNDS)
}

/// SIGKILL group `pgid` through `host`, in at most `rounds` kill rounds; a
/// group already gone is not an error.
///
/// Under parallel load macOS can refuse `killpg` with `EPERM` for a group that
/// holds a zombie, and then signal no member (larry#131: 7 of 24 parallel
/// runs; on an idle Mac the same kill succeeds). So an ended leader not yet
/// reaped could shield its whole group. On `EPERM` each live member is killed
/// by pid, round by round, until the group holds none: the unreaped leader
/// still pins the group id, so no pid read here can be a stranger's.
#[cfg(any(target_os = "macos", all(any(test, kani), unix)))]
fn end_group_on(host: &mut impl GroupHost, pgid: i32, rounds: usize) -> io::Result<()> {
    match host.kill_group(pgid) {
        Ok(()) => return Ok(()),
        Err(error) if is_gone(&error) => return Ok(()),
        Err(error) if error.raw_os_error() != Some(rustix::io::Errno::PERM.raw_os_error()) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, pgid, "end_group: the group refused its kill");
            return Err(error);
        }
        Err(shielded) => trace_error(&shielded, "a zombie shields its group; killed by pid"),
    }
    for _ in 0..rounds {
        let mut live = false;
        for (pid, zombie) in host.members(pgid)? {
            if zombie {
                continue;
            }
            live = true;
            if let Err(error) = host.kill_pid(pid)
                && !is_gone(&error)
            {
                #[cfg(feature = "trace")]
                crate::trace::debug!(%error, pid, "end_group: a live member refused its kill");
                return Err(error);
            }
        }
        if !live {
            return Ok(());
        }
    }
    // A fixed message, so the Kani proof does not model a formatter; the trace
    // names the group and the bound.
    let refusal = io::Error::other("a process group still had live members after every kill round");
    #[cfg(feature = "trace")]
    crate::trace::debug!(%refusal, pgid, rounds, "end_group: a group outlived every round");
    Err(refusal)
}

/// The OS code that says a kill's target no longer exists: `ESRCH`, or the
/// Windows `ERROR_INVALID_PARAMETER` for an id no process holds.
#[cfg(unix)]
const GONE: i32 = rustix::io::Errno::SRCH.raw_os_error();
/// See the Unix [`GONE`].
#[cfg(windows)]
const GONE: i32 = crate::process::windows::ERROR_INVALID_PARAMETER;
/// See the Unix [`GONE`]: a target that starts no process has no such code.
#[cfg(not(any(unix, windows)))]
const GONE: i32 = i32::MIN;

/// Whether `error` says the target no longer exists.
fn is_gone(error: &io::Error) -> bool {
    error.raw_os_error() == Some(GONE)
}

/// Block until child `pid` has ended, leaving it unreaped:
/// `waitid(P_PID, pid, WEXITED | WNOWAIT)`, retried on `EINTR`.
#[cfg(unix)]
fn wait_exit_keeping(pid: i32) -> io::Result<()> {
    let target = rustix::process::Pid::from_raw(pid).ok_or_else(crate::process::invalid_pid)?;
    loop {
        match rustix::process::waitid(
            rustix::process::WaitId::Pid(target),
            rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT,
        ) {
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => {}
            Err(errno) => {
                let error = crate::process::errno_to_io(errno);
                #[cfg(feature = "trace")]
                crate::trace::debug!(%error, pid, "wait: waitid refused the leader");
                return Err(error);
            }
        }
    }
}

/// Every byte of `stream`, or nothing when there is no stream.
fn read_all(stream: Option<impl io::Read>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        stream.read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

/// A reader thread's result; a reader that panicked is an error.
fn joined(
    outcome: Result<io::Result<Vec<u8>>, Box<dyn std::any::Any + Send>>,
) -> io::Result<Vec<u8>> {
    match outcome {
        Ok(read) => read,
        Err(_payload) => Err(io::Error::other("a child output reader panicked")),
    }
}

/// Name a refusal where it is made; nothing else sees it.
pub(super) fn trace_error(error: &io::Error, what: &'static str) {
    #[cfg(feature = "trace")]
    crate::trace::debug!(%error, what, "process::Command: a refusal");
    #[cfg(not(feature = "trace"))]
    let _named = (error, what);
}
