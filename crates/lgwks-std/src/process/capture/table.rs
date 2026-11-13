//! The process-table readings behind the capture and the running check.
//!
//! Linux reads `/proc`, macOS one `sysctl`, and every other Unix one `ps`
//! spawn; Windows one Toolhelp snapshot. Each reading answers the same two
//! questions, who is whose parent and who is a zombie, and the bounded walk
//! turns the first answer into a [`DescendantSet`]. A child of `capture`, so
//! it builds the set it returns.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

#[cfg(target_os = "linux")]
use super::MAX_CAPTURED_THREADS;
use super::{ContainmentMechanism, DescendantSet, MAX_CAPTURED_DEPTH, MAX_CAPTURED_DESCENDANTS};

/// `procfs(5)`'s state letter for a process that has exited and awaits its reap.
///
/// This and the pure `/proc` parsers below also build in every unix test, so
/// the macOS suite drives them too; only Linux calls them in production.
#[cfg(any(target_os = "linux", all(test, unix)))]
const PROC_ZOMBIE: char = 'Z';

/// `PF_EXITING` in the task flags `/proc/<pid>/stat` records (field 9).
///
/// The kernel sets it as `do_exit` begins, before the process releases its
/// memory and files and long before it becomes a zombie. A process carrying it
/// never returns to user code: it has been stopped and is finishing its exit.
#[cfg(any(target_os = "linux", all(test, unix)))]
const PF_EXITING: u64 = 0x0000_0004;

/// The task flags' position among the `stat` fields after the command name:
/// field 9, counted from the state letter (field 3).
#[cfg(any(target_os = "linux", all(test, unix)))]
const FLAGS_FIELD_AFTER_NAME: usize = 6;

/// The `stat` line of `pid`, or `None` when it is unreadable.
#[cfg(all(target_os = "linux", feature = "process"))]
pub(super) fn read_proc_stat(pid: i32) -> Option<String> {
    let path = std::path::Path::new("/proc")
        .join(pid.to_string())
        .join("stat");
    std::fs::read_to_string(path).ok()
}

/// Whether a `/proc/<pid>/stat` line describes a process still running user
/// code: not a zombie, and not part way through its exit.
///
/// A line that cannot be parsed reports running, the answer that understates
/// what a cleanup achieved.
#[cfg(any(target_os = "linux", all(test, unix)))]
pub(in crate::process) fn proc_stat_says_running(stat: &str) -> bool {
    // `pid (comm) state …`: `comm` may itself contain spaces and parentheses, so
    // the fields start after the **last** `)`.
    let Some(fields) = stat
        .rfind(')')
        .and_then(|end| stat.get(end.saturating_add(1)..))
    else {
        return true;
    };
    let mut fields = fields.split_ascii_whitespace();
    let Some(state) = fields.next().and_then(|field| field.chars().next()) else {
        return true;
    };
    if state == PROC_ZOMBIE {
        return false;
    }
    let flags = fields
        .nth(FLAGS_FIELD_AFTER_NAME.saturating_sub(1))
        .and_then(|field| field.parse::<u64>().ok());
    !matches!(flags, Some(flags) if flags & PF_EXITING != 0)
}

/// Whether `pid` has a `SIGKILL` delivered that it has not yet acted on.
///
/// A killed process stays in state `R` or `S` until the scheduler next runs it,
/// and on a loaded host that is long enough for a cleanup to observe it
/// "running" and report a survivor that ignored every signal — which a
/// `SIGKILL` cannot be. The kernel records the pending kill in the thread's and
/// the process's pending masks (`SigPnd`, `ShdPnd` in `/proc/<pid>/status`); a
/// process with it set never returns to user code. An unreadable status claims
/// nothing.
#[cfg(any(target_os = "linux", all(test, unix)))]
pub(super) fn kill_is_pending(pid: i32) -> bool {
    let path = std::path::Path::new("/proc")
        .join(pid.to_string())
        .join("status");
    match std::fs::read_to_string(path) {
        Ok(status) => status_has_kill_pending(&status),
        Err(error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(error = ?error, pid, "an unreadable status claims no pending kill");
            #[cfg(not(feature = "trace"))]
            drop(error);
            false
        }
    }
}

/// Whether a `/proc/<pid>/status` text carries `SIGKILL` in a pending mask.
#[cfg(any(target_os = "linux", all(test, unix)))]
pub(in crate::process) fn status_has_kill_pending(status: &str) -> bool {
    // A signal's bit in the masks is its number less one.
    let kill_bit = rustix::process::Signal::KILL
        .as_raw()
        .checked_sub(1)
        .and_then(|bit| u32::try_from(bit).ok())
        .and_then(|bit| 1u64.checked_shl(bit));
    let Some(kill_bit) = kill_bit else {
        return false;
    };
    status
        .lines()
        .filter_map(|line| {
            line.strip_prefix("SigPnd:")
                .or_else(|| line.strip_prefix("ShdPnd:"))
        })
        .filter_map(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .any(|mask| mask & kill_bit != 0)
}

/// Whether each process in the table is a zombie, in one table read (macOS).
///
/// One `sysctl(KERN_PROC_ALL)` syscall (#345): the kernel's own `p_stat`, with
/// no process spawn, where the `ps` reading cost a fork and an exec per call.
#[cfg(all(target_os = "macos", feature = "process"))]
pub(super) fn read_table_zombies() -> io::Result<BTreeMap<i32, bool>> {
    Ok(crate::process::darwin::table()?
        .into_iter()
        .map(|row| (row.pid, row.zombie))
        .collect())
}

/// Whether each process in the table is a zombie, from one `ps` read.
///
/// The reading for a Unix with neither a per-process state file nor a kernel
/// table this crate reads directly.
#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "macos"),
    unix,
    feature = "process"
))]
pub(super) fn read_table_zombies() -> io::Result<BTreeMap<i32, bool>> {
    let mut states = BTreeMap::new();
    for row in read_ps_table(&["pid=", "stat="], "read_table_zombies")? {
        let mut columns = row.split_ascii_whitespace();
        let (Some(pid), Some(stat)) = (columns.next(), columns.next()) else {
            continue;
        };
        let Ok(pid) = pid.parse::<i32>() else {
            continue;
        };
        states.insert(pid, stat.starts_with('Z'));
    }
    Ok(states)
}

/// The rows `ps -A -o <columns…>` printed, one [`String`] per process.
///
/// The one place this crate shells out to read a process table, so the shape of
/// that read is one decision rather than one per column set: the columns are
/// data, `caller` names the reading in the refusal so an operator reading the log
/// knows which of the two reads failed, and a non-zero exit is a refusal rather
/// than an empty table — an empty reading here would say "this host has no
/// processes" when the truth is "the reading failed". macOS reads its table
/// through `sysctl` and Linux through `/proc` instead (#345).
#[cfg(all(
    unix,
    not(target_os = "macos"),
    not(target_os = "linux"),
    feature = "process"
))]
fn read_ps_table(columns: &[&str], caller: &'static str) -> io::Result<Vec<String>> {
    let mut arguments = vec!["-A"];
    for column in columns {
        arguments.push("-o");
        arguments.push(column);
    }
    let output = crate::process::run_ps(&arguments, caller)?;
    if !output.status.success() {
        let refusal = Err(io::Error::other(format!(
            "lgwks_std::process ({caller}): the process table could not be read with \
             `ps`, so no process outside the supervised group could be named",
        )));
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "read_ps_table: returning an error to the caller");
        return refusal;
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Capture the tree by reading each process's own child list (Linux).
///
/// Returns `None` — rather than an error — when the kernel exposes no such list,
/// because "this kernel has no per-process child list" is a fact about the
/// platform and not a failure to read one: the caller falls back to the whole
/// table, which every Linux userland has.
#[cfg(all(target_os = "linux", feature = "process"))]
pub(super) fn capture_proc_children(root: i32) -> Option<DescendantSet> {
    // The probe is the root's own list: if this process has none, a kernel
    // without `CONFIG_PROC_CHILDREN` has none for anybody.
    read_proc_children(root).ok()?;
    Some(walk_descendants(
        root,
        ContainmentMechanism::ProcChildrenTree,
        |pid| match read_proc_children(pid) {
            Ok(children) => Some(children),
            // A process that exited between being named and being read has no
            // list any more, and its children now belong to init: nothing under
            // it is reachable through this root, which is the same answer an
            // empty list gives.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
            Err(error) => {
                #[cfg(feature = "trace")]
                crate::trace::debug!(
                    pid,
                    error = %error,
                    "descendant capture: a child list could not be read"
                );
                #[cfg(not(feature = "trace"))]
                let _unreported = error;
                None
            }
        },
    ))
}

/// The child pids `/proc/<pid>/task/<tid>/children` names, across its threads.
///
/// # Errors
///
/// `NotFound` when the process is gone. Any list this cannot read whole is an
/// error too, never a short list: a kernel without `CONFIG_PROC_CHILDREN`
/// (`Unsupported`), an unreadable thread, or more threads than the bound.
#[cfg(all(target_os = "linux", feature = "process"))]
pub(in crate::process) fn read_proc_children(pid: i32) -> io::Result<Vec<i32>> {
    read_children_under(std::path::Path::new("/proc"), pid, MAX_CAPTURED_THREADS)
}

/// [`read_proc_children`] over the proc tree at `proc_root`, reading at most
/// `max_threads` threads.
///
/// A short list here reads as "no more children", which makes a supervisor
/// miss a descendant it should stop. So the only list it returns is a whole
/// one; a thread that exited between the listing and its read is the one
/// thread it may skip, since its list went with it.
#[cfg(any(target_os = "linux", all(test, unix)))]
fn read_children_under(
    proc_root: &std::path::Path,
    pid: i32,
    max_threads: usize,
) -> io::Result<Vec<i32>> {
    let task_dir = proc_root.join(pid.to_string()).join("task");
    let mut children = Vec::new();
    let mut threads = 0_usize;
    for entry in std::fs::read_dir(task_dir)? {
        let entry = entry?;
        threads = threads.saturating_add(1);
        if threads > max_threads {
            let refusal = Err(io::Error::other(format!(
                "read_proc_children: pid {pid} has more than {max_threads} threads, so its child list would be partial"
            )));
            #[cfg(feature = "trace")]
            crate::trace::debug!(error = ?refusal.as_ref().err(), "read_proc_children: returning an error to the caller");
            return refusal;
        }
        let thread = entry.path();
        match std::fs::read_to_string(thread.join("children")) {
            Ok(text) => children.extend(
                text.split_ascii_whitespace()
                    .filter_map(|pid| pid.parse::<i32>().ok()),
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !thread.exists() => {}
            Err(error) => {
                // A live thread with no list is a kernel that keeps none, which
                // must not read as "the process is gone".
                let kind = match error.kind() {
                    io::ErrorKind::NotFound => io::ErrorKind::Unsupported,
                    other => other,
                };
                let refusal = Err(io::Error::new(
                    kind,
                    format!(
                        "read_proc_children: pid {pid}'s child list could not be read: {error}"
                    ),
                ));
                #[cfg(feature = "trace")]
                crate::trace::debug!(error = ?refusal.as_ref().err(), "read_proc_children: returning an error to the caller");
                return refusal;
            }
        }
    }
    Ok(children)
}

/// Walk the tree breadth-first from `root` through `children_of`, bounded in
/// depth, in breadth and in retained ids.
///
/// `children_of` answers `Some(children)` for a list it read (empty when the
/// parent has none) and `None` for one it could not read; a `None` marks the set
/// as a prefix, since what is below that parent is unknown.
///
/// One walk for both readings, because the bounds are the contract and a bound
/// duplicated across two copies is two bounds that drift: the depth limit is
/// what stops a fork-without-bound program, the retained-id limit is what stops
/// a host with many processes from making a capture grow, and both report
/// themselves through [`DescendantSet::is_truncated`] rather than by returning
/// short.
#[cfg(all(any(unix, windows), feature = "process"))]
fn walk_descendants(
    root: i32,
    mechanism: ContainmentMechanism,
    mut children_of: impl FnMut(i32) -> Option<Vec<i32>>,
) -> DescendantSet {
    let mut set = DescendantSet {
        mechanism,
        pids: Vec::new(),
        truncated: false,
    };
    let mut frontier = vec![root];
    let mut visited: BTreeSet<i32> = BTreeSet::from([root]);
    for _ in 0..MAX_CAPTURED_DEPTH {
        let mut next = Vec::new();
        for parent in frontier {
            let Some(listed) = children_of(parent) else {
                // A parent whose list could not be read may have children this
                // walk will never see, so the set is a prefix of the tree.
                set.truncated = true;
                continue;
            };
            for child in listed {
                if child <= 0 || !visited.insert(child) {
                    continue;
                }
                if set.pids.len() >= MAX_CAPTURED_DESCENDANTS {
                    set.truncated = true;
                    return finish(set);
                }
                set.pids.push(child);
                next.push(child);
            }
        }
        if next.is_empty() {
            return finish(set);
        }
        frontier = next;
    }
    // The depth bound ran out with processes still below it: a prefix, not a tree.
    set.truncated = true;
    finish(set)
}

/// Sort and deduplicate a captured id list.
#[cfg(all(any(unix, windows), feature = "process"))]
fn finish(mut set: DescendantSet) -> DescendantSet {
    set.pids.sort_unstable();
    set.pids.dedup();
    set
}

/// Every `(pid, ppid)` pair of the host's process table, in one read.
///
/// macOS: one `sysctl(KERN_PROC_ALL)` syscall (#345), with no process spawn.
#[cfg(all(target_os = "macos", feature = "process"))]
fn parent_pairs() -> io::Result<Vec<(i32, i32)>> {
    Ok(crate::process::darwin::table()?
        .into_iter()
        .map(|row| (row.pid, row.ppid))
        .collect())
}

/// Every `(pid, ppid)` pair of the host's process table, read from `/proc`.
///
/// Linux: one `/proc/<pid>/stat` read per process, with no process spawn
/// (#345). This is the fallback the per-process child walk takes when the
/// root's own `/proc` entry is gone (it exited, or never existed); before, that
/// fallback spawned `ps`, which a host without `ps` refused. A pid that exits
/// between the listing and its read has no row, which is what it is.
#[cfg(all(target_os = "linux", feature = "process"))]
fn parent_pairs() -> io::Result<Vec<(i32, i32)>> {
    let mut pairs = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if let Some(parent) = read_proc_stat(pid).as_deref().and_then(stat_parent) {
            pairs.push((pid, parent));
        }
    }
    Ok(pairs)
}

/// The parent pid a `/proc/<pid>/stat` line names: field 4, the second after
/// the command name, which may itself hold spaces and parentheses.
#[cfg(all(target_os = "linux", feature = "process"))]
pub(in crate::process) fn stat_parent(stat: &str) -> Option<i32> {
    stat.rfind(')')
        .and_then(|end| stat.get(end.saturating_add(1)..))
        .and_then(|fields| fields.split_ascii_whitespace().nth(1))
        .and_then(|parent| parent.parse::<i32>().ok())
}

/// Every `(pid, ppid)` pair of the host's process table, from one `ps` read.
///
/// The reading for a Unix that is neither Linux nor macOS.
#[cfg(all(
    unix,
    not(target_os = "macos"),
    not(target_os = "linux"),
    feature = "process"
))]
fn parent_pairs() -> io::Result<Vec<(i32, i32)>> {
    let mut pairs = Vec::new();
    for row in read_ps_table(&["pid=", "ppid="], "capture_table_snapshot")? {
        let mut columns = row.split_ascii_whitespace();
        let (Some(pid), Some(parent)) = (columns.next(), columns.next()) else {
            continue;
        };
        if let (Ok(pid), Ok(parent)) = (pid.parse::<i32>(), parent.parse::<i32>()) {
            pairs.push((pid, parent));
        }
    }
    Ok(pairs)
}

/// Every `(pid, ppid)` pair of the host's process table, from one Toolhelp
/// snapshot (Windows), the stranger filter already applied.
#[cfg(all(windows, feature = "process"))]
fn parent_pairs() -> io::Result<Vec<(i32, i32)>> {
    Ok(
        crate::process::windows::snapshot(&crate::process::windows::NATIVE)?
            .into_iter()
            .filter_map(|(pid, parent)| {
                Some((i32::try_from(pid).ok()?, i32::try_from(parent).ok()?))
            })
            .collect(),
    )
}

/// Capture the tree from one `pid`/`ppid` snapshot of the whole process table.
///
/// The portable reading, and the one every Unix without a per-process child list
/// uses. It reads one row per process on the host — one `sysctl` on macOS,
/// `/proc` on Linux, one `ps` invocation elsewhere — and it races more than the
/// per-process walk: a
/// process that forks while the snapshot is taken is a row the snapshot cannot
/// contain. The walk is therefore tried first wherever one exists, and this is
/// the fallback and the default everywhere else.
#[cfg(all(any(unix, windows), feature = "process"))]
pub(super) fn capture_table_snapshot(root: i32) -> io::Result<DescendantSet> {
    Ok(snapshot_from(root, parent_pairs()?))
}

/// The tree under `root` that one table's `(pid, ppid)` rows describe.
///
/// A row whose pid is not positive names no process and is skipped, never a
/// reason to stop reading the rest.
#[cfg(all(any(unix, windows), feature = "process"))]
fn snapshot_from(root: i32, pairs: Vec<(i32, i32)>) -> DescendantSet {
    let mut children_of: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for (pid, parent) in pairs {
        if pid <= 0 {
            continue;
        }
        children_of.entry(parent).or_default().push(pid);
    }
    walk_descendants(
        root,
        ContainmentMechanism::ProcessTableSnapshot,
        // The snapshot holds every row on the host, so a parent with no row
        // naming it has no children.
        |pid| Some(children_of.remove(&pid).into_iter().flatten().collect()),
    )
}

#[cfg(all(test, unix))]
mod tests {
    //! The moved readings, driven on every unix test host: the pure `/proc`
    //! parsers, the bounded walk and the snapshot fold.

    use super::{
        kill_is_pending, proc_stat_says_running, read_children_under, snapshot_from,
        status_has_kill_pending, walk_descendants,
    };
    use crate::process::{ContainmentMechanism, DescendantSet};
    use std::io;

    /// A fake `/proc` with pid 7's threads: each `(tid, Some(list))` holds a
    /// `children` file, and each `(tid, None)` is a thread with no such file.
    fn proc_tree(threads: &[(i32, Option<&str>)]) -> io::Result<tempfile::TempDir> {
        let root = tempfile::tempdir()?;
        for &(tid, list) in threads {
            let thread = root.path().join("7/task").join(tid.to_string());
            std::fs::create_dir_all(&thread)?;
            if let Some(list) = list {
                std::fs::write(thread.join("children"), list)?;
            }
        }
        Ok(root)
    }

    /// The bug this catches (#965 review, finding 5.1): a kernel without
    /// `CONFIG_PROC_CHILDREN` has a task dir but no `children` file. The old
    /// reader skipped the thread and returned an empty list, so the probe
    /// passed and the capture never fell back to the whole table.
    #[test]
    fn a_live_thread_with_no_list_is_unsupported_not_empty() -> io::Result<()> {
        let tree = proc_tree(&[(7, None)])?;
        let Err(error) = read_children_under(tree.path(), 7, 64) else {
            return Err(io::Error::other(
                "a missing list must not read as no children",
            ));
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        Ok(())
    }

    /// The bug this catches: more threads than the bound broke out of the loop
    /// and returned the lists read so far as if whole.
    #[test]
    fn more_threads_than_the_bound_is_an_error_not_a_prefix() -> io::Result<()> {
        let tree = proc_tree(&[(7, Some("8")), (9, Some("10")), (11, Some("12"))])?;
        assert!(
            read_children_under(tree.path(), 7, 2).is_err(),
            "three threads over a bound of two is a partial read"
        );
        let mut whole = read_children_under(tree.path(), 7, 3)?;
        whole.sort_unstable();
        assert_eq!(whole, [8, 10, 12], "at the bound every list is read");
        Ok(())
    }

    #[test]
    fn a_gone_process_is_not_found_and_bad_entries_are_skipped() -> io::Result<()> {
        let tree = proc_tree(&[(7, Some("8 x -1 9\n"))])?;
        assert_eq!(read_children_under(tree.path(), 7, 64)?, [8, -1, 9]);
        let Err(error) = read_children_under(tree.path(), 99, 64) else {
            return Err(io::Error::other("a pid with no task dir has no list"));
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        Ok(())
    }

    /// A walk from pid 1 over `tree`, where `None` is a list that could not
    /// be read and a parent with no row has no children.
    fn walk(tree: &[(i32, Option<&[i32]>)]) -> DescendantSet {
        walk_descendants(
            1,
            ContainmentMechanism::ProcessTableSnapshot,
            |parent| match tree.iter().find(|&&(pid, _)| pid == parent) {
                Some(&(_, children)) => children.map(<[i32]>::to_vec),
                None => Some(Vec::new()),
            },
        )
    }

    #[test]
    fn a_stat_line_says_running_unless_zombie_or_exiting() {
        assert!(
            proc_stat_says_running("7 (sh) S 1 7 7 0 -1 0"),
            "a sleeper runs"
        );
        assert!(
            !proc_stat_says_running("7 (sh) Z 1 7 7 0 -1 0"),
            "a zombie does not"
        );
        assert!(
            !proc_stat_says_running("7 (sh) R 1 7 7 0 -1 4"),
            "PF_EXITING does not"
        );
        assert!(
            proc_stat_says_running("7 (sh)"),
            "a line with no state claims running"
        );
        assert!(
            proc_stat_says_running("garbage"),
            "a line with no name claims running"
        );
    }

    #[test]
    fn a_pending_kill_in_either_mask_is_read() {
        let thread = "SigPnd:\t0000000000000100\nShdPnd:\t0000000000000000\n";
        assert!(
            status_has_kill_pending(thread),
            "SIGKILL pending on the thread"
        );
        let shared = "SigPnd:\t0000000000000000\nShdPnd:\t0000000000000100\n";
        assert!(
            status_has_kill_pending(shared),
            "SIGKILL pending on the process"
        );
        let other = "SigPnd:\t0000000000000002\nShdPnd:\t0000000000000000\n";
        assert!(!status_has_kill_pending(other), "SIGINT is not a kill");
        assert!(!status_has_kill_pending("Name:\tsh\n"), "no mask, no kill");
    }

    #[test]
    fn a_pid_with_no_status_claims_no_pending_kill() {
        assert!(!kill_is_pending(i32::MAX), "no status file claims nothing");
    }

    #[test]
    /// The bugs this catches: a non-positive child or a repeat that ends the
    /// list, and an unreadable list that ends the whole level.
    fn the_walk_skips_bad_children_and_marks_an_unreadable_list() {
        let set = walk(&[(1, Some(&[0, -3, 2, 2, 3])), (2, None), (3, Some(&[4]))]);
        assert_eq!(set.pids(), &[2, 3, 4], "every real descendant, once");
        assert!(
            set.is_truncated(),
            "an unreadable list makes the set a prefix"
        );
        let whole = walk(&[(1, Some(&[2])), (2, Some(&[3]))]);
        assert_eq!(whole.pids(), &[2, 3]);
        assert!(!whole.is_truncated(), "a fully read tree is whole");
    }

    #[test]
    fn a_chain_past_the_depth_bound_is_a_prefix() {
        let set = walk_descendants(1, ContainmentMechanism::ProcessTableSnapshot, |parent| {
            Some(vec![parent.saturating_add(1)])
        });
        assert!(
            set.is_truncated(),
            "the depth bound ran out with processes below it"
        );
    }

    #[test]
    fn a_snapshot_skips_rows_that_name_no_process() {
        let set = snapshot_from(1, vec![(0, 1), (-2, 1), (5, 1), (6, 5), (9, 8)]);
        assert_eq!(
            set.pids(),
            &[5, 6],
            "the tree under the root, bad rows skipped"
        );
        assert!(!set.is_truncated());
    }
}
