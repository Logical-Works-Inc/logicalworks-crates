//! The process census: every process a subreaper's child left behind, stopped
//! and named (ADR 0082, INV-CI-ORPHANS-1, #1128 G2b).
//!
//! A test wrapper that is its test's subreaper adopts every orphan the test
//! makes. [`reap_until`] waits for the test as init waits for anything: each
//! adoptee that ends meanwhile is reaped at once, so a `kill -0` on it answers
//! as it would under init. [`stop_orphans`] then kills, names and reaps every
//! process still adopted, round by round, because a killed parent's children
//! are adopted next.

use std::fmt;
use std::io;

/// How many kill rounds a sweep takes before it calls the tree unstoppable: a
/// tree that adopts a fresh child every round is a fork bomb, not a leak.
pub(crate) const MAX_ROUNDS: usize = 64;

/// Survivors a verdict names; the rest are counted.
const NAMED: usize = 16;

/// What a sweep reads and does, by pid: the kernel, or a sim's model.
pub(crate) trait Host {
    /// This process's children, live or ended, sorted.
    fn children(&mut self) -> io::Result<Vec<i32>>;
    /// `pid`'s name while it runs; `None` once it has ended.
    fn live_name(&mut self, pid: i32) -> Option<String>;
    /// Send `pid` SIGKILL.
    fn kill(&mut self, pid: i32) -> io::Result<()>;
    /// Reap one ended child, waiting for one to end; `None` when none is left.
    fn reap_any(&mut self) -> io::Result<Option<i32>>;
}

/// One process a test left running, stopped by the sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Survivor {
    /// Its pid.
    pid: i32,
    /// Its name, as the kernel reports it.
    name: String,
}

#[cfg(test)]
impl Survivor {
    /// Its pid.
    pub(crate) const fn pid(&self) -> i32 {
        self.pid
    }
}

/// What a sweep found: the survivors it stopped, and why it could not stop
/// the rest. Clean is no survivor and no refusal; its `Display` is the
/// verdict a test's log carries.
///
/// ```no_run
/// let left: lgwks_std::process::Orphans = lgwks_std::process::stop_orphans();
/// println!("{left}");
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Orphans {
    /// Each process alive when a round found it, in the order found.
    survivors: Vec<Survivor>,
    /// Why the sweep stopped short, when it did.
    unstopped: Option<String>,
}

impl Orphans {
    /// Each process the sweep found alive and stopped.
    #[cfg(test)]
    pub(crate) fn survivors(&self) -> &[Survivor] {
        &self.survivors
    }

    /// Why the sweep could not stop every process, when it could not.
    #[cfg(test)]
    pub(crate) fn unstopped(&self) -> Option<&str> {
        self.unstopped.as_deref()
    }

    /// Whether the test left nothing running and the sweep was not refused:
    /// exactly when the `Display` verdict is empty.
    #[cfg(test)]
    pub(crate) fn is_clean(&self) -> bool {
        self.survivors.is_empty() && self.unstopped.is_none()
    }
}

impl fmt::Display for Orphans {
    /// `the test left N live processes: pid (name) …`, then any refusal.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.survivors.is_empty() {
            write!(
                formatter,
                "the test left {} live processes:",
                self.survivors.len()
            )?;
            for survivor in self.survivors.iter().take(NAMED) {
                write!(formatter, " {} ({})", survivor.pid, survivor.name)?;
            }
            let more = self.survivors.len().saturating_sub(NAMED);
            if more > 0 {
                write!(formatter, " and {more} more")?;
            }
        }
        if let Some(ref why) = self.unstopped {
            if !self.survivors.is_empty() {
                writeln!(formatter)?;
            }
            write!(
                formatter,
                "the census could not stop the test's processes: {why}"
            )?;
        }
        Ok(())
    }
}

/// Kill, name and reap every child of this process, round by round, until a
/// listing is empty, a step is refused, or [`MAX_ROUNDS`] pass.
pub(crate) fn sweep(host: &mut impl Host) -> Orphans {
    let mut found = Orphans::default();
    for _ in 0..MAX_ROUNDS {
        let children = match host.children() {
            Ok(children) => children,
            Err(error) => {
                found.unstopped = Some(format!("the child list was unreadable: {error}"));
                return found;
            }
        };
        if children.is_empty() {
            return found;
        }
        for pid in children.iter().copied() {
            if let Some(name) = host.live_name(pid) {
                found.survivors.push(Survivor { pid, name });
            }
            if let Err(error) = host.kill(pid) {
                found.unstopped = Some(format!("{pid} refused SIGKILL: {error}"));
                return found;
            }
        }
        // One reap per child killed: each call returns once some child ends,
        // and every child this round killed will, so none of them waits on a
        // child nobody signalled.
        for _ in &children {
            match host.reap_any() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    found.unstopped = Some(format!("a reap was refused: {error}"));
                    return found;
                }
            }
        }
    }
    found.unstopped = Some(format!(
        "children were still being adopted after {MAX_ROUNDS} rounds"
    ));
    found
}

/// The kernel, as a sweep sees it: this process's children under the
/// subreaper flag.
#[cfg(all(target_os = "linux", feature = "process"))]
struct Kernel;

#[cfg(all(target_os = "linux", feature = "process"))]
impl Host for Kernel {
    fn children(&mut self) -> io::Result<Vec<i32>> {
        super::own_children()
    }

    fn live_name(&mut self, pid: i32) -> Option<String> {
        // A dead test's lifeline is adopted here and ends on its own once it
        // has released what the test held; the sweep still kills it, and what
        // it held is judged by name in its own right (ADR 0087).
        if is_lifeline(pid) {
            return None;
        }
        let stat = super::capture::table::read_proc_stat(pid)?;
        name_if_live(&stat, super::capture::table::kill_is_pending(pid))
    }

    fn kill(&mut self, pid: i32) -> io::Result<()> {
        super::kill_process(pid)
    }

    fn reap_any(&mut self) -> io::Result<Option<i32>> {
        Ok(wait_any()?.map(|(pid, _)| pid))
    }
}

/// Whether `pid` is a lifeline, by the `argv[0]` it was started with. An
/// unreadable command line is not one, so the process is judged by name.
#[cfg(all(target_os = "linux", feature = "process"))]
fn is_lifeline(pid: i32) -> bool {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(line) => is_lifeline_cmdline(&line),
        Err(error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, pid, "census: a command line was unreadable; the process is judged by name");
            #[cfg(not(feature = "trace"))]
            drop(error);
            false
        }
    }
}

/// Whether a `/proc/<pid>/cmdline` (NUL-separated) starts with the lifeline's
/// `argv[0]`.
#[cfg(any(
    all(target_os = "linux", feature = "process"),
    all(test, unix, feature = "process")
))]
pub(crate) fn is_lifeline_cmdline(line: &[u8]) -> bool {
    line.split(|byte| *byte == 0).next() == Some(super::lifeline::LIFELINE_NAME.as_bytes())
}

/// The name a `/proc/<pid>/stat` line gives a process still running user code;
/// `None` for a zombie, an exiting process, or one with `SIGKILL` pending: a
/// process the test killed just before it exited is already ending.
#[cfg(any(all(target_os = "linux", feature = "process"), all(test, unix)))]
pub(crate) fn name_if_live(stat: &str, kill_pending: bool) -> Option<String> {
    if !super::capture::table::proc_stat_says_running(stat) || kill_pending {
        return None;
    }
    // `pid (comm) state …`: the name may hold spaces and parentheses.
    let open = stat.find('(')?.checked_add(1)?;
    let close = stat.rfind(')')?;
    stat.get(open..close).map(str::to_owned)
}

/// Waits through `wait` until child `pid` ends and returns its status; every
/// other child `wait` reports ended meanwhile is passed over, already reaped.
///
/// # Errors
///
/// `NotFound` when `wait` runs out of children first, and `wait`'s own error.
pub(crate) fn wait_for<Status>(
    pid: i32,
    mut wait: impl FnMut() -> io::Result<Option<(i32, Status)>>,
) -> io::Result<Status> {
    loop {
        match wait()? {
            Some((ended, status)) if ended == pid => return Ok(status),
            Some(_) => {}
            None => {
                let refusal = Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("reap_until: {pid} is not a child of this process"),
                ));
                #[cfg(feature = "trace")]
                crate::trace::debug!(error = ?refusal.as_ref().err(), "reap_until: returning an error to the caller");
                return refusal;
            }
        }
    }
}

/// Wait for any child of this process to end and reap it: init's one call.
/// `None` when no child is left.
#[cfg(all(target_os = "linux", feature = "process"))]
fn wait_any() -> io::Result<Option<(i32, std::process::ExitStatus)>> {
    use std::os::unix::process::ExitStatusExt;
    loop {
        match rustix::process::wait(rustix::process::WaitOptions::empty()) {
            Ok(Some((pid, status))) => {
                let status = std::process::ExitStatus::from_raw(status.as_raw());
                return Ok(Some((pid.as_raw_nonzero().get(), status)));
            }
            Ok(None) | Err(rustix::io::Errno::CHILD) => return Ok(None),
            Err(rustix::io::Errno::INTR) => {}
            Err(errno) => {
                let refusal = Err(super::errno_to_io(errno));
                #[cfg(feature = "trace")]
                crate::trace::debug!(error = ?refusal.as_ref().err(), "wait_any: returning an error to the caller");
                return refusal;
            }
        }
    }
}

/// Wait for the child `pid` to end and return its status, reaping every other
/// child that ends meanwhile, as init reaps what it adopts.
///
/// A subreaper that waited only for its own child would hold each ended
/// adoptee as a zombie, which a `kill -0` still reports present, so a program
/// waiting for its orphan to go would wait for ever. Call
/// [`enable_child_subreaper`](super::enable_child_subreaper) before starting
/// `pid`.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// lgwks_std::process::enable_child_subreaper()?;
/// let test = std::process::Command::new("true").spawn()?;
/// let pid = i32::try_from(test.id()).map_err(std::io::Error::other)?;
/// let status = lgwks_std::process::reap_until(pid)?;
/// assert!(status.success());
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// `NotFound` when `pid` is not, or is no longer, a child of this process,
/// and the wait's own error otherwise.
#[cfg(all(target_os = "linux", feature = "process"))]
pub fn reap_until(pid: i32) -> io::Result<std::process::ExitStatus> {
    wait_for(pid, wait_any)
}

/// Kill, name and reap every process this subreaper still holds: what a
/// program it ran left behind.
///
/// Round by round, since a killed parent's own children are adopted next; a
/// tree still adopting after 64 rounds, or a refused step, is named in the
/// verdict rather than read as clean.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// lgwks_std::process::enable_child_subreaper()?;
/// let test = std::process::Command::new("sh").args(["-c", "sleep 300 &"]).spawn()?;
/// let pid = i32::try_from(test.id()).map_err(std::io::Error::other)?;
/// lgwks_std::process::reap_until(pid)?;
/// let left = lgwks_std::process::stop_orphans();
/// assert!(!left.to_string().is_empty(), "the background sleep was left");
/// # Ok(())
/// # }
/// ```
#[cfg(all(target_os = "linux", feature = "process"))]
#[must_use]
pub fn stop_orphans() -> Orphans {
    sweep(&mut Kernel)
}
