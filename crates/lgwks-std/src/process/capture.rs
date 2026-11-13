//! Descendant capture, the running check, and the single-pid signals.
//!
//! The process-table readings they rest on live in the child module `table`.
//!
//! A process group is the floor, not the tree. A descendant that calls `setsid`
//! has left the group by construction, so no group signal reaches it, and the
//! only way to stop one is to name its pid. These primitives are that naming:
//! read the process table to find every process descended from a root, signal
//! each of them by pid, and observe which of them are still present.
//!
//! Both halves are deliberately narrow. The capture is bounded by
//! [`MAX_CAPTURED_DESCENDANTS`] and reports itself truncated past it, because a
//! process table is a property of the host rather than of the caller's tree; and
//! it is a **snapshot taken while the root is alive**, because that is the only
//! moment a descendant still names it as a parent. A descendant orphaned before
//! the capture has been adopted by the platform's init and is no longer reachable
//! through the root — the caller is told which mechanism ran, and a caller that
//! needs that case closed needs a kernel-level owner (a cgroup, or a Windows job
//! object), neither of which this crate has.

use std::collections::BTreeSet;
use std::io;

#[cfg(all(not(unix), feature = "process"))]
use super::unsupported;
#[cfg(all(unix, feature = "process"))]
use super::{errno_to_io, invalid_pid, pid_from_raw};

/// The process-table readings: `/proc`, `sysctl`, `ps` and Toolhelp.
#[cfg(all(any(unix, windows), feature = "process"))]
pub(super) mod table;
/// The stat and status parsers the process unit tests drive, at the path they use.
#[cfg(all(test, target_os = "linux", feature = "process"))]
pub(super) use table::{proc_stat_says_running, stat_parent, status_has_kill_pending};

/// The most process ids one descendant capture retains.
///
/// A bound on the process table rather than on the caller's tree: without one, a
/// host with many processes could make a supervisor's memory grow with the host
/// rather than with the tree it started. Past the bound the capture keeps the
/// first [`MAX_CAPTURED_DESCENDANTS`] ids and reports
/// [`DescendantSet::is_truncated`], so a caller learns it is holding a prefix
/// instead of trusting it as the whole tree.
pub const MAX_CAPTURED_DESCENDANTS: usize = 4096;

/// How deep one descendant capture walks before it stops descending.
///
/// A descendant tree deeper than this is a program that forks without bound,
/// which the capture reports as truncated rather than following for ever.
#[cfg(any(unix, windows))]
const MAX_CAPTURED_DEPTH: usize = 64;

/// How many thread directories of one process the Linux walk reads.
///
/// `/proc/<pid>/task/<tid>/children` attributes a child to the thread that
/// forked it, so a threaded process needs every one of its thread directories
/// read before its child list is complete. The bound keeps a process with
/// thousands of threads from turning one capture into thousands of reads; a tree
/// past it is reported truncated.
#[cfg(all(target_os = "linux", feature = "process"))]
const MAX_CAPTURED_THREADS: usize = 64;

/// How a descendant capture read the process table.
///
/// The receipt names this, because "every process the supervisor started is
/// gone" is a claim about a mechanism and not only about an outcome: the same
/// green result means different things depending on whether the table was read
/// per-process or in one snapshot, or not read at all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum ContainmentMechanism {
    /// No process table was read, so nothing outside the group was captured.
    ///
    /// The honest report when this target exposes no table, or when the read
    /// failed. A caller that needs the stronger claim has to say so rather than
    /// read an empty capture as "there were no descendants".
    #[default]
    ProcessGroupOnly,
    /// Linux: each process's own child list, from `/proc`.
    ///
    /// One read per process in the tree rather than one per process on the
    /// host, which is what keeps a cleanup cheap under concurrency. It needs a
    /// kernel built with `CONFIG_PROC_CHILDREN`; a kernel without it falls back
    /// to [`Self::ProcessTableSnapshot`], and a host with neither reports
    /// [`Self::ProcessGroupOnly`].
    ProcChildrenTree,
    /// A `pid`/`ppid` snapshot of the whole process table.
    ///
    /// The portable reading, used on every Unix without a per-process child list
    /// and as the fallback when that list cannot be read. On macOS it is one
    /// `sysctl(KERN_PROC)` syscall and on Linux one `/proc/<pid>/stat` read per
    /// process (#345); on any other Unix it costs one `ps` invocation. Either
    /// way it reads one row per process on the host, so it is
    /// the slower of the two readings and the one a high-concurrency cleanup pays
    /// most for.
    ProcessTableSnapshot,
    /// Linux: the tree's cgroup v2 scope, stopped by `cgroup.kill`.
    ///
    /// No process table was read for the members this stopped: the kernel owns
    /// the membership, so a `setsid` escapee attached to the scope dies with it
    /// and no fork race applies to the members it holds. The completeness claim
    /// rests on the scope reading empty afterwards (`CgroupScope::members`),
    /// not on a table snapshot.
    CgroupKill,
    /// Linux: descendants the subreaper flag re-parented to the supervisor.
    ///
    /// An orphan adopted by this process is found as its child rather than
    /// through a walk from a leader that may already have exited, and a pid
    /// that is both captured and adopted is provably still the same process.
    /// The completeness claim rests on that intersection
    /// (`adopted_descendants`), not on the group alone.
    SubreaperAdoption,
    /// Windows: the tree's job object, stopped by `TerminateJobObject`.
    ///
    /// The kernel owns the membership (`ProcessJob::members`), so a descendant
    /// that outlived its parent is still in the job and dies with it, and no
    /// snapshot race applies to the members it holds.
    JobObject,
}

impl ContainmentMechanism {
    /// The stronger of two mechanisms, for a capture that merged both.
    ///
    /// A kernel-level owner beats a table reading (it stops what no snapshot
    /// can name), a per-process walk beats a whole-table snapshot (it reads
    /// less and races less), and either beats no capture at all. Merging never
    /// reports the weaker mechanism of the two, because a caller reading the
    /// merged value would otherwise be told less than the capture actually
    /// established.
    #[must_use]
    pub const fn strongest(self, other: Self) -> Self {
        match (self, other) {
            (Self::CgroupKill, _) | (_, Self::CgroupKill) => Self::CgroupKill,
            (Self::JobObject, _) | (_, Self::JobObject) => Self::JobObject,
            (Self::SubreaperAdoption, _) | (_, Self::SubreaperAdoption) => Self::SubreaperAdoption,
            (Self::ProcChildrenTree, _) | (_, Self::ProcChildrenTree) => Self::ProcChildrenTree,
            (Self::ProcessTableSnapshot, _) | (_, Self::ProcessTableSnapshot) => {
                Self::ProcessTableSnapshot
            }
            (Self::ProcessGroupOnly, Self::ProcessGroupOnly) => Self::ProcessGroupOnly,
        }
    }

    /// `false` only for [`Self::ProcessGroupOnly`]: an empty capture under it
    /// says nothing about what descends from the root, while an empty reading
    /// under any other mechanism is evidence — a table snapshot, a per-process
    /// walk, an emptied cgroup scope, or an adoption intersection — rather
    /// than the absence of one.
    #[must_use]
    pub const fn read_a_table(self) -> bool {
        !matches!(self, Self::ProcessGroupOnly)
    }
}

/// Every process found descended from one root, and whether the walk was whole.
///
/// Sorted and deduplicated, so two captures of the same tree compare equal and a
/// merge is a set union. The root itself is **not** in the set: it leads the
/// group, and a caller that wants it stopped signals the group.
///
/// `is_truncated` is the difference between "this is the tree" and "this is the
/// prefix of the tree the capture had room for". A caller that needs the whole
/// tree must read it; a caller that must not signal a stranger can only trust
/// the set it holds, so the flag is reported rather than absorbed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct DescendantSet {
    /// How the ids were found.
    pub(super) mechanism: ContainmentMechanism,
    /// The captured ids, sorted and deduplicated.
    pub(super) pids: Vec<i32>,
    /// Whether a bound stopped the walk before the tree ended.
    pub(super) truncated: bool,
}

impl DescendantSet {
    /// How the ids were found.
    #[must_use]
    pub const fn mechanism(&self) -> ContainmentMechanism {
        self.mechanism
    }

    /// The captured ids, sorted and deduplicated, excluding the root.
    #[must_use]
    pub fn pids(&self) -> &[i32] {
        &self.pids
    }

    /// How many ids the capture holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pids.len()
    }

    /// Whether the capture holds no ids.
    ///
    /// **Not** a claim that the root had no descendants: a capture that could
    /// not read the table is empty too, and
    /// [`Self::mechanism`] is what tells the two apart.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pids.is_empty()
    }

    /// Whether `pid` is one of the captured ids.
    #[must_use]
    pub fn contains(&self, pid: i32) -> bool {
        self.pids.binary_search(&pid).is_ok()
    }

    /// Whether a bound stopped the walk before the tree ended.
    ///
    /// `true` means the set is a **prefix** of the tree, so a caller that must
    /// account for every descendant has to treat the difference as unaccounted
    /// rather than as absent.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Fold another capture of the same root into this one.
    ///
    /// The union is what makes the capture cumulative across a cleanup's
    /// rounds: a fork that completed between two captures is added by the
    /// later one, and the merged mechanism is the strongest of the two so the
    /// receipt never understates what was read. Ids past
    /// [`MAX_CAPTURED_DESCENDANTS`] are refused by the bound and the merged set
    /// reports [`Self::is_truncated`].
    pub fn absorb(&mut self, other: &Self) {
        self.mechanism = self.mechanism.strongest(other.mechanism);
        self.truncated = self.truncated || other.truncated;
        for pid in &other.pids {
            match self.pids.binary_search(pid) {
                Ok(_) => {}
                Err(at) if self.pids.len() < MAX_CAPTURED_DESCENDANTS => {
                    self.pids.insert(at, *pid);
                }
                Err(_) => self.truncated = true,
            }
        }
        // Re-sorted rather than assumed: the union is only sorted if every half
        // was, and an invariant the caller cannot establish is not an invariant.
        self.pids.sort_unstable();
        self.pids.dedup();
    }
}

/// Every process descended from `root` at the moment of the call.
///
/// The root must be positive: `0` names the caller's own position in a pid walk
/// and a negative id names no process, so both are refused as
/// [`std::io::ErrorKind::InvalidInput`] before anything is read.
///
/// # What the result is and is not
///
/// It is a **snapshot of the parent relation** taken while `root` was alive, and
/// it says nothing about processes that were orphaned before the call: a
/// descendant whose own parent has already exited has been adopted by the
/// platform's init, so it no longer names `root` and cannot be reached from it.
/// On Linux `PR_SET_CHILD_SUBREAPER` would re-parent such an orphan to *this*
/// process instead, which makes it findable — and unattributable, because a
/// process that a supervisor never observed under its own root cannot be proved
/// to have come from that root rather than from another supervisor in the same
/// process. This function therefore reads the tree and does not adopt orphans;
/// the mechanism is reported so a caller can say which claim it is making.
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidInput`] for a non-positive `root`, and the
/// reading's own error when no process table could be read at all — a caller that
/// receives an error has captured nothing and must fall back to signalling the
/// group alone rather than read the absence as an empty tree.
#[cfg(all(unix, feature = "process"))]
pub fn capture_descendants(root: i32) -> io::Result<DescendantSet> {
    if root <= 0 {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "capture_descendants: returning an error to the caller");
        return refusal;
    }
    #[cfg(target_os = "linux")]
    if let Some(set) = table::capture_proc_children(root) {
        return Ok(set);
    }
    table::capture_table_snapshot(root)
}

/// Every process descended from `root` at the moment of the call.
///
/// Windows: one Toolhelp snapshot, with each pair whose child predates its
/// parent dropped as a reused id (see `windows::snapshot`). Windows keeps a
/// process's recorded parent after the parent exits, so unlike Unix an orphan
/// is still named by its dead parent's id; the stranger filter is what stops a
/// reissued id adopting it.
#[cfg(all(windows, feature = "process"))]
pub fn capture_descendants(root: i32) -> io::Result<DescendantSet> {
    super::windows::pid_u32(root)?;
    table::capture_table_snapshot(root)
}

/// Every process descended from `root` at the moment of the call.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a fabricated empty tree,
/// because an empty set and a tree nobody looked for are different facts.
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn capture_descendants(root: i32) -> io::Result<DescendantSet> {
    let _ = root;
    Err(unsupported("process capture_descendants is Unix-only"))
}

/// Send SIGKILL to the single process `pid`.
///
/// The per-process counterpart of [`super::kill_process_group`], for a descendant that
/// left the group: a group signal cannot reach it and a pid can. It carries the
/// same obligation as every signal here — signal only a pid this supervisor owns
/// — and the same hazard, which is why a caller reads [`process_exists`] or a
/// fresh capture rather than reusing a pid it observed earlier.
///
/// `pid` must be positive; a non-positive id is refused before any signal leaves
/// the process, because `kill(0, …)` would signal the caller's own group.
#[cfg(all(unix, feature = "process"))]
pub fn kill_process(pid: i32) -> io::Result<()> {
    if pid <= 0 {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "kill_process: returning an error to the caller");
        return refusal;
    }
    let target = pid_from_raw(pid)?;
    rustix::process::kill_process(target, rustix::process::Signal::KILL).map_err(errno_to_io)
}

/// Terminate the single process `pid`.
///
/// An id no process holds is `ERROR_INVALID_PARAMETER`, the Windows `ESRCH`; a
/// process that already exited counts as stopped.
#[cfg(all(windows, feature = "process"))]
pub fn kill_process(pid: i32) -> io::Result<()> {
    super::windows::terminate(&super::windows::NATIVE, super::windows::pid_u32(pid)?)
}

/// Send SIGKILL to the single process `pid`.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`].
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn kill_process(pid: i32) -> io::Result<()> {
    let _ = pid;
    Err(unsupported("process kill_process is Unix-only"))
}

/// Send SIGCONT to the single process `pid`, resuming it if it is stopped.
///
/// The release half of the gated start: the gate shell stops itself before it
/// execs the payload, and this resumes it once the caller has recorded the
/// leader's identity. A SIGCONT to a running process is a harmless no-op, so
/// a release that lands after the payload already runs changes nothing — but
/// a release is only sent after the gate is observed stopped (see
/// [`super::child_stopped_without_reaping`]), which is what keeps a racing release
/// from being lost before the child stops itself.
///
/// `pid` must be positive; a non-positive id is refused before any signal
/// leaves the process.
#[cfg(all(unix, feature = "process"))]
pub fn continue_process(pid: i32) -> io::Result<()> {
    if pid <= 0 {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "continue_process: returning an error to the caller");
        return refusal;
    }
    let target = pid_from_raw(pid)?;
    rustix::process::kill_process(target, rustix::process::Signal::CONT).map_err(errno_to_io)
}

/// Send SIGCONT to the single process `pid`.
///
/// The `process` capability's gate is Unix; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a release that runs
/// nothing.
#[cfg(all(not(unix), feature = "process"))]
pub fn continue_process(pid: i32) -> io::Result<()> {
    let _ = pid;
    Err(unsupported("process continue_process is Unix-only"))
}

/// Whether `pid` currently names a process.
///
/// Signal zero against a single process, so it delivers nothing and can be used
/// on a process this supervisor does not own without disturbing it — which is
/// what makes it the observation half of a descendant sweep. `EPERM` means the
/// process exists and this process may not signal it, so it counts as present;
/// `ESRCH` means no process holds the id.
///
/// The answer is about **this number at this instant**, not about a process
/// observed earlier: an id the OS has reissued answers for whoever holds it now.
/// A caller that needs "the process I captured" rather than "this number" must
/// keep the id pinned until it has finished signalling — see
/// [`super::child_has_exited_without_reaping`] for how the supervisor pins its leader.
///
/// `pid` must be positive; a non-positive id is refused as
/// [`std::io::ErrorKind::InvalidInput`].
#[cfg(all(unix, feature = "process"))]
pub fn process_exists(pid: i32) -> io::Result<bool> {
    if pid <= 0 {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "process_exists: returning an error to the caller");
        return refusal;
    }
    let target = pid_from_raw(pid)?;
    match rustix::process::test_kill_process(target) {
        Ok(()) | Err(rustix::io::Errno::PERM) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(errno) => Err(errno_to_io(errno)),
    }
}

/// Whether `pid` currently names a process: one that runs, one that exited
/// while something holds its object, or one this process may not open.
#[cfg(all(windows, feature = "process"))]
pub fn process_exists(pid: i32) -> io::Result<bool> {
    super::windows::exists(&super::windows::NATIVE, super::windows::pid_u32(pid)?)
}

/// Whether `pid` currently names a process.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`].
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn process_exists(pid: i32) -> io::Result<bool> {
    let _ = pid;
    Err(unsupported("process process_exists is Unix-only"))
}

/// Every pid in `pids` that is still running, read in one pass of the table.
///
/// The batch form of [`process_exists`], and the difference between *present* and
/// *running* is the whole reason it exists. A process that has exited and is
/// waiting to be reaped still holds its id — `kill -0` on it succeeds, and it is
/// still a member of its group — so a cleanup that reported such a pid as a
/// survivor would report every shell that forked one child as a leak, and a test
/// that waited for its absence would wait for a reaper this crate does not own.
/// A zombie has run to its end: it occupies an id and does no work.
///
/// On Linux two more states are not running, because neither returns to user
/// code: a process part way through its exit (`PF_EXITING`), and one with a
/// `SIGKILL` delivered that the scheduler has not yet run it to act on. On a
/// loaded host a killed process can sit in the second state for longer than a
/// cleanup's rounds take, and reporting it as a survivor would name a pid that
/// ignored a signal nobody can ignore.
///
/// The conservative direction is the conservative one: a pid the table says
/// exists and whose state cannot be read is reported **running**, because a
/// receipt that claims less than it established is the honest one and a receipt
/// that claims more is the dangerous one.
///
/// Off Linux the states come from one whole-table snapshot — a `sysctl` on
/// macOS, a `ps` spawn elsewhere — so a signal-zero probe runs first and a set
/// whose every pid is already absent returns empty without reading anything. A
/// probe is not a state: a pid the probe finds present is still decided by the
/// table, and a pid that exits between the probe and the snapshot is decided the
/// way the table reads it. Absence needs no table: no live process can name a
/// pid nobody holds as its parent, and a zombie holds its id, so an absent pid
/// is not running however the table would have spelled it.
///
/// `pids` may be empty, in which case no table is read and the answer is empty:
/// asking about nothing must not cost a table read.
#[cfg(all(unix, feature = "process"))]
pub fn running_processes(pids: &[i32]) -> io::Result<BTreeSet<i32>> {
    if pids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut running = BTreeSet::new();
    #[cfg(target_os = "linux")]
    for pid in pids {
        match table::read_proc_stat(*pid) {
            Some(stat) => {
                if table::proc_stat_says_running(&stat) && !table::kill_is_pending(*pid) {
                    running.insert(*pid);
                }
            }
            // The state file was unreadable: claim nothing and report running,
            // which is the answer that understates what is still there.
            None => {
                if process_exists(*pid)? {
                    running.insert(*pid);
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // The probe before the table (#345): signal zero costs a syscall and
        // answers absence exactly. A pid nobody holds is not running — a
        // zombie would hold its id — so a set the probe proves wholly absent
        // needs no table. A pid the probe finds present is still decided by
        // the table, which is what tells a zombie from a process that runs.
        let mut present = Vec::with_capacity(pids.len());
        for pid in pids {
            if process_exists(*pid)? {
                present.push(*pid);
            }
        }
        if present.is_empty() {
            return Ok(running);
        }
        let zombies = table::read_table_zombies()?;
        for pid in present {
            match zombies.get(&pid).copied() {
                Some(true) => {}
                Some(false) => {
                    running.insert(pid);
                }
                None => {
                    if process_exists(pid)? {
                        running.insert(pid);
                    }
                }
            }
        }
    }
    Ok(running)
}

/// Every pid in `pids` that is still running: its exit code is `STILL_ACTIVE`.
///
/// An exited process whose object is still held is not running, like a zombie.
/// A process this process may not open is reported running, the answer that
/// claims less.
#[cfg(all(windows, feature = "process"))]
pub fn running_processes(pids: &[i32]) -> io::Result<BTreeSet<i32>> {
    let mut running = BTreeSet::new();
    for pid in pids {
        if super::windows::running(&super::windows::NATIVE, super::windows::pid_u32(*pid)?)? {
            running.insert(*pid);
        }
    }
    Ok(running)
}

/// Every pid in `pids` that is still running, read in one pass of the table.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`].
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn running_processes(pids: &[i32]) -> io::Result<BTreeSet<i32>> {
    let _ = pids;
    Err(unsupported("process running_processes is Unix-only"))
}
