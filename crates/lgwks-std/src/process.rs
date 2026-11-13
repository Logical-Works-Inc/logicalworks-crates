//! Process-group termination and observation for supervised subprocesses.
//!
//! Stable `std` can kill a direct child (`Child::kill`) but cannot signal a
//! whole process group, so killing a shell leaves orphaned grandchildren.
//! It also cannot observe a child's exit without reaping it, which is the
//! primitive a supervisor needs to keep a group leader's id from being
//! reused under it: a zombie leader still holds its pid, so its group id
//! cannot be reissued until the supervisor has finished signalling. These
//! are the `process` feature's primitives. Unix and Windows; other targets
//! report [`std::io::ErrorKind::Unsupported`] rather than a fabricated success.
//! Windows has no process groups, so a "group" there is the tree below a root
//! process (`kill_process_group` stops the root and everything the process
//! table names as descended from it), and `ProcessJob` is its kernel-level
//! owner, the counterpart of a cgroup.
//!
//! The module is split by responsibility: this file holds the group and child
//! primitives, `capture` the descendant capture and the running check,
//! `identity` process identities and orphaned-group reaping, `cgroup` the
//! Linux kernel-level containment, `darwin` the one macOS process-table read
//! (a `sysctl` syscall) and `windows` the `kernel32` calls; those two hold the
//! only `unsafe` in the workspace (INV-STD-UNSAFE-1, #345, #263).

use std::io;

/// Descendant capture, the running check, and the process-table readings.
mod capture;
/// Batch descendant capture across multiple roots (#798).
mod capture_many;
/// cgroup v2 scopes and the child subreaper (Linux).
#[cfg(target_os = "linux")]
mod cgroup;
/// The macOS process table through one `sysctl(KERN_PROC)` syscall (#345).
#[cfg(target_os = "macos")]
mod darwin;
/// Process identities and orphaned-group reaping.
mod identity;
/// What a zero-timeout wait on a Windows process handle answers: pure, so
/// every host's tests judge it, not only a Windows runner's.
#[cfg(any(windows, test))]
mod wait;
/// Windows process table, identity, termination and job objects (#263).
#[cfg(windows)]
mod windows;

pub use capture::{
    ContainmentMechanism, DescendantSet, MAX_CAPTURED_DESCENDANTS, capture_descendants,
    continue_process, kill_process, process_exists, running_processes,
};

#[doc(hidden)]
impl DescendantSet {
    pub(crate) fn from_raw_parts(
        mechanism: ContainmentMechanism,
        pids: Vec<i32>,
        truncated: bool,
    ) -> Self {
        Self {
            mechanism,
            pids,
            truncated,
        }
    }
}
#[doc(hidden)]
pub use capture_many::{MAX_CAPTURE_ROOTS, capture_descendants_many};
#[cfg(target_os = "linux")]
pub use cgroup::{
    CgroupScope, adopted_descendants, enable_child_subreaper, own_children, reap_descendants,
};
pub use identity::{
    MAX_START_TOKEN_BYTES, OrphanReap, ProcessIdentity, ProcessIdentityError, identify_process,
    reap_orphaned_group,
};
#[cfg(windows)]
pub use windows::ProcessJob;

/// Send SIGKILL to every process in group `pgid`.
///
/// `pgid` is a process-group id as reported by the OS (a positive integer;
/// typically the leader's pid). An invalid id surfaces the OS error rather than
/// succeeding silently. This kills unconditionally (SIGKILL cannot be
/// caught); callers that need graceful shutdown must signal SIGTERM
/// themselves before falling back here.
#[cfg(all(unix, feature = "process"))]
pub fn kill_process_group(pgid: i32) -> io::Result<()> {
    // Reject before touching rustix: 0 names the CALLER's group (SIGKILL
    // would hit us) and negatives panic inside Pid::from_raw in debug.
    if pgid <= 0 {
        let refusal = Err(invalid_pgid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "kill_process_group: returning an error to the caller");
        return refusal;
    }
    let pid = pid_from_raw(pgid)?;
    rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).map_err(errno_to_io)
}

/// Stop the process `pgid` and every process descended from it.
///
/// Windows has no process groups, so the group is the tree: the descendants the
/// process table names under `pgid` are captured, `pgid` itself is terminated
/// first (a dead root spawns nothing more), then every descendant. A refusal
/// never ends the sweep: every descendant is tried, and the first refusal is
/// returned after the last (#913 item 1). A capture that failed still stops the
/// root and is then reported. A tree that left through a parent that had already
/// exited is not reachable from the root; [`ProcessJob`] is the owner that
/// reaches it. An id no process holds is an error (`ERROR_INVALID_PARAMETER`,
/// the Windows form of `ESRCH`).
#[cfg(all(windows, feature = "process"))]
pub fn kill_process_group(pgid: i32) -> io::Result<()> {
    let root = windows::pid_u32(pgid)?;
    let captured = capture_descendants(pgid);
    let root_stopped = windows::terminate(&windows::NATIVE, root);
    let descendants = match captured.as_ref() {
        Ok(set) => set.pids(),
        Err(_) => &[],
    };
    let swept = signal_every(descendants.iter().copied(), |pid| match windows::terminate(
        &windows::NATIVE,
        windows::pid_u32(pid)?,
    ) {
        Err(error) if error.raw_os_error() == Some(windows::ERROR_INVALID_PARAMETER) => Ok(()),
        other => other,
    });
    root_stopped?;
    captured?;
    swept
}

/// Signal every target in order, past any refusal, and answer the first
/// refusal once the last target has been tried.
///
/// A tree stop that returns at its first refusal leaves every later target
/// running, which is the containment failure #913 item 1 names: a refusal is a
/// fact to report, never a reason to stop signalling. `signal` answers `Ok`
/// for a target that had already stopped; every other error counts.
#[cfg(all(any(unix, windows), feature = "process"))]
fn signal_every<Target: Copy + std::fmt::Debug>(
    targets: impl IntoIterator<Item = Target>,
    mut signal: impl FnMut(Target) -> io::Result<()>,
) -> io::Result<()> {
    let mut first_refusal: Option<io::Error> = None;
    let mut refusals = 0_usize;
    for target in targets {
        if let Err(error) = signal(target) {
            refusals = refusals.saturating_add(1);
            #[cfg(feature = "trace")]
            crate::trace::debug!(?target, %error, "signal_every: a target refused; the sweep goes on");
            if first_refusal.is_none() {
                first_refusal = Some(error);
            }
        }
    }
    match first_refusal {
        Some(error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(refusals, %error, "signal_every: returning the first refusal after the sweep");
            #[cfg(not(feature = "trace"))]
            let _counted = refusals;
            Err(error)
        }
        None => Ok(()),
    }
}

/// Send SIGKILL to every process in group `pgid`.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a fabricated success.
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn kill_process_group(pgid: i32) -> io::Result<()> {
    let _ = pgid;
    Err(unsupported("process kill_process_group is Unix-only"))
}

/// Whether `pgid` currently names a process group.
///
/// This is `kill(-pgid, 0)`: signal zero checks the group without delivering
/// anything to it. `EPERM` means the group exists but this process may not
/// signal it, so it counts as present; `ESRCH` means no process is in it. A
/// supervisor confirms a cleanup with this after `kill_process_group`, so a
/// group that cannot be observed is reported as an error, never as absent.
///
/// `pgid` must be positive: 0 names the caller's own group and a negative id
/// names a process, so both are refused as [`std::io::ErrorKind::InvalidInput`].
#[cfg(all(unix, feature = "process"))]
pub fn process_group_exists(pgid: i32) -> io::Result<bool> {
    if pgid <= 0 {
        let refusal = Err(invalid_pgid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "process_group_exists: returning an error to the caller");
        return refusal;
    }
    let pid = pid_from_raw(pgid)?;
    match rustix::process::test_kill_process_group(pid) {
        Ok(()) | Err(rustix::io::Errno::PERM) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(errno) => Err(errno_to_io(errno)),
    }
}

/// Whether the tree under `pgid` still has a process: the root holds its id, or
/// a process the table names as its descendant is running.
#[cfg(all(windows, feature = "process"))]
pub fn process_group_exists(pgid: i32) -> io::Result<bool> {
    let root = windows::pid_u32(pgid)?;
    if windows::exists(&windows::NATIVE, root)? {
        return Ok(true);
    }
    Ok(!running_processes(capture_descendants(pgid)?.pids())?.is_empty())
}

/// Whether `pgid` currently names a process group.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a fabricated answer.
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn process_group_exists(pgid: i32) -> io::Result<bool> {
    let _ = pgid;
    Err(unsupported("process process_group_exists is Unix-only"))
}

/// Whether child `pid` has exited, leaving it un-reaped.
///
/// This is `waitid(P_PID, pid, WEXITED | WSTOPPED | WCONTINUED | WNOHANG |
/// WNOWAIT)`: it reports the child's state *without reaping it*, so a
/// following `Child::wait` still returns the real exit status. That gap —
/// exited but not reaped — is what a supervisor needs: an unreaped (zombie)
/// leader still occupies its pid, so the group id cannot be reused by the OS
/// while the supervisor still owes signals to it. Reaping is what releases
/// the id, and this call never reaps.
///
/// Only an exited child — normally, by signal, or with a core dump — counts
/// as exited. A stopped or continued child is alive, and a waitid that only
/// asked for exits would misread the stop the gated start depends on as an
/// exit and have the supervisor kill a child that merely waited (#691).
///
/// `Ok(false)` means running or stopped; `Ok(true)` means exited and still
/// waitable.
#[cfg(all(unix, feature = "process"))]
pub fn child_has_exited_without_reaping(pid: i32) -> io::Result<bool> {
    Ok(wait_child_without_reaping(pid)?
        .is_some_and(|state| state.exited() || state.killed() || state.dumped()))
}

/// The child's waitid state without reaping it, shared by the exit and stop
/// observations: one validation, one full-mask wait, and the state for the
/// caller to judge by its own `si_code`.
#[cfg(all(unix, feature = "process"))]
fn wait_child_without_reaping(pid: i32) -> io::Result<Option<rustix::process::WaitIdStatus>> {
    if pid <= 0 {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "wait_child_without_reaping: returning an error to the caller");
        return refusal;
    }
    let Some(target) = rustix::process::Pid::from_raw(pid) else {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "wait_child_without_reaping: returning an error to the caller");
        return refusal;
    };
    rustix::process::waitid(
        rustix::process::WaitId::Pid(target),
        rustix::process::WaitIdOptions::EXITED
            | rustix::process::WaitIdOptions::STOPPED
            | rustix::process::WaitIdOptions::CONTINUED
            | rustix::process::WaitIdOptions::NOHANG
            | rustix::process::WaitIdOptions::NOWAIT,
    )
    .map_err(errno_to_io)
}

/// Whether process `pid` has exited while something still holds its object.
///
/// A held process object keeps the id from being reissued, which is the Windows
/// form of an unreaped child. An id no process holds is an error, never `true`.
#[cfg(all(windows, feature = "process"))]
pub fn child_has_exited_without_reaping(pid: i32) -> io::Result<bool> {
    windows::exited(&windows::NATIVE, windows::pid_u32(pid)?)
}

/// Whether child `pid` has exited, leaving it un-reaped.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a fabricated success.
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn child_has_exited_without_reaping(pid: i32) -> io::Result<bool> {
    let _ = pid;
    Err(unsupported(
        "process child_has_exited_without_reaping is Unix-only",
    ))
}

/// Whether child `pid` is stopped, leaving it un-reaped.
///
/// This is `waitid(P_PID, pid, WEXITED | WSTOPPED | WCONTINUED | WNOHANG |
/// WNOWAIT)`: it reports the child's state *without reaping it and without
/// changing it*, so a following SIGCONT resumes exactly the stop observed
/// here. Only a stopped child counts: an exited one is answered by
/// [`child_has_exited_without_reaping`], and a continued one is running.
/// Asking for every state keeps the waitid from waiting on the one state the
/// caller did not name.
///
/// `Ok(false)` means running, continued, or exited; `Ok(true)` means stopped
/// and still waitable.
#[cfg(all(unix, feature = "process"))]
pub fn child_stopped_without_reaping(pid: i32) -> io::Result<bool> {
    Ok(wait_child_without_reaping(pid)?.is_some_and(|state| state.stopped()))
}

/// Whether child `pid` is stopped, leaving it un-reaped.
///
/// The `process` capability's gate is Unix; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a fabricated answer.
#[cfg(all(not(unix), feature = "process"))]
pub fn child_stopped_without_reaping(pid: i32) -> io::Result<bool> {
    let _ = pid;
    Err(unsupported(
        "process child_stopped_without_reaping is Unix-only",
    ))
}

/// Clear `FD_CLOEXEC` on `file_des`, allowing it to be inherited across `exec`.
///
/// # Errors
///
/// An [`io::Error`] if the `fcntl` syscall fails.
#[doc(hidden)]
#[cfg(all(unix, feature = "process"))]
pub fn clear_cloexec(file_des: std::os::fd::BorrowedFd<'_>) -> io::Result<()> {
    match rustix::io::fcntl_setfd(file_des, rustix::io::FdFlags::empty()) {
        Ok(()) => Ok(()),
        Err(error) => {
            let io_err = io::Error::from(error);
            #[cfg(feature = "trace")]
            crate::trace::debug!(error = ?io_err, "clear_cloexec: fcntl_setfd failed");
            Err(io_err)
        }
    }
}

/// Set `FD_CLOEXEC` on `file_des`, preventing inheritance across `exec`.
///
/// # Errors
///
/// An [`io::Error`] if the `fcntl` syscall fails.
#[doc(hidden)]
#[cfg(all(unix, feature = "process"))]
pub fn set_cloexec(file_des: std::os::fd::BorrowedFd<'_>) -> io::Result<()> {
    match rustix::io::fcntl_setfd(file_des, rustix::io::FdFlags::CLOEXEC) {
        Ok(()) => Ok(()),
        Err(error) => {
            let io_err = io::Error::from(error);
            #[cfg(feature = "trace")]
            crate::trace::debug!(error = ?io_err, "set_cloexec: fcntl_setfd failed");
            Err(io_err)
        }
    }
}

/// The directories `ps` is looked up in: the system's own, never the caller's.
///
/// `ps` output names the pids a supervisor signals and the start tokens it
/// trusts, so a `ps` found through an inherited `PATH`, or run with an
/// inherited `LD_PRELOAD` or `DYLD_INSERT_LIBRARIES`, could steer a kill (#913
/// item 9, CWE-426). Every unix this crate builds for keeps `ps` in one of
/// these two directories.
#[cfg(all(unix, feature = "process"))]
const PS_SEARCH_PATH: &str = "/bin:/usr/bin";

/// Run `ps` with `arguments`, in UTC under the C locale, and return what it printed.
///
/// The one place this crate starts `ps`, shared by the table readings and the
/// start token. The child gets a cleared environment holding only
/// [`PS_SEARCH_PATH`], `TZ` and `LC_ALL`: nothing the caller inherited picks
/// the binary or loads into it, and a date column reads the same in every
/// caller's time zone and language, which a start token compared across two
/// processes depends on. `caller` names the reading in the refusal, and the
/// error's kind is kept so a missing `ps` stays distinguishable from a refused
/// one.
#[cfg(all(unix, feature = "process"))]
fn run_ps(arguments: &[&str], caller: &'static str) -> io::Result<std::process::Output> {
    run_ps_in(std::ffi::OsStr::new(PS_SEARCH_PATH), arguments, caller)
}

/// [`run_ps`] with `ps` looked up in `search_path`, so a test can show the
/// lookup follows the given path and nothing else.
#[cfg(all(unix, feature = "process"))]
fn run_ps_in(
    search_path: &std::ffi::OsStr,
    arguments: &[&str],
    caller: &'static str,
) -> io::Result<std::process::Output> {
    // `Command` searches the child's `PATH` when one is set on it, so the
    // fixed path, not the parent's, picks the binary.
    let spawned = std::process::Command::new("ps")
        .args(arguments)
        .env_clear()
        .env("PATH", search_path)
        .env("TZ", "UTC0")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match spawned {
        Ok(output) => Ok(output),
        Err(error) => {
            let refusal = Err(io::Error::new(
                error.kind(),
                format!("lgwks_std::process ({caller}): `ps` could not be run: {error}"),
            ));
            #[cfg(feature = "trace")]
            crate::trace::debug!(error = ?refusal.as_ref().err(), "run_ps: returning an error to the caller");
            refusal
        }
    }
}

#[cfg(all(test, unix, feature = "process"))]
mod ps_path_tests {
    //! `ps` runs from the fixed system path with a cleared environment, so a
    //! caller's `PATH` or preload cannot pick what drives a kill (#913 item 9).

    use std::collections::BTreeSet;
    use std::io;
    use std::os::unix::fs::PermissionsExt as _;

    use super::{run_ps, run_ps_in};

    /// The variables the child may hold: the three `run_ps_in` sets, and the
    /// ones a POSIX shell adds for itself.
    const ALLOWED: [&str; 7] = ["PATH", "TZ", "LC_ALL", "PWD", "OLDPWD", "SHLVL", "_"];

    /// A directory holding a planted `ps` that runs `body` under `/bin/sh`.
    fn planted_ps(body: &str) -> io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("ps");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        Ok(dir)
    }

    #[test]
    /// The bug this catches: `ps` looked up through the parent's `PATH`, so
    /// the planted binary is ignored here and a hostile one wins in the field.
    fn the_lookup_follows_the_given_path_and_nothing_else() -> io::Result<()> {
        let planted = planted_ps("echo planted")?;
        let output = run_ps_in(planted.path().as_os_str(), &[], "test")?;
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "planted");
        Ok(())
    }

    #[test]
    /// The bug this catches: an inherited environment, where `LD_PRELOAD` or
    /// `DYLD_INSERT_LIBRARIES` loads code into the `ps` that names the pids.
    fn the_child_inherits_no_variable_from_the_caller() -> io::Result<()> {
        let planted = planted_ps("/usr/bin/env")?;
        let output = run_ps_in(planted.path().as_os_str(), &[], "test")?;
        let text = String::from_utf8_lossy(&output.stdout);
        let held: BTreeSet<&str> = text
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        for name in ["PATH", "TZ", "LC_ALL"] {
            assert!(held.contains(name), "{name} is set for the child: {text}");
        }
        let leaked: Vec<&str> = held
            .iter()
            .copied()
            .filter(|name| !ALLOWED.contains(name))
            .collect();
        assert!(leaked.is_empty(), "the child inherited {leaked:?}");
        Ok(())
    }

    #[test]
    fn the_system_path_finds_the_real_ps() -> io::Result<()> {
        let own = std::process::id().to_string();
        let output = run_ps(&["-o", "pid=", "-p", &own], "test")?;
        assert!(output.status.success(), "the system `ps` ran");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), own);
        Ok(())
    }
}

#[cfg(all(unix, feature = "process"))]
/// Convert a rustix errno into the std error surface.
fn errno_to_io(errno: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(errno.raw_os_error())
}

#[cfg(all(unix, feature = "process"))]
/// Convert a positive raw process-group id into rustix's checked pid type.
fn pid_from_raw(pgid: i32) -> io::Result<rustix::process::Pid> {
    rustix::process::Pid::from_raw(pgid).ok_or_else(invalid_pgid)
}

#[cfg(all(unix, feature = "process"))]
/// Construct the invalid-process-group-id error.
fn invalid_pgid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "process group id must be a positive pid",
    )
}

#[cfg(all(any(unix, windows), feature = "process"))]
/// Construct the invalid-process-id error.
fn invalid_pid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "process id must be a positive pid",
    )
}

/// What a failed cgroup scope `mkdir` under `mount` tells the caller.
///
/// A missing, unwritable or read-only mount is a fact about the host, so it
/// becomes [`io::ErrorKind::Unsupported`] and the supervisor falls back to the
/// capture rounds. `AlreadyExists` stands as itself: the directory belongs to
/// another tree, live or dead, and is never adopted, empty or not (#913 item
/// 8). Any other error stands too. It lives here, not in the Linux-only
/// `cgroup` module, so every unix test host runs its tests.
#[cfg(all(feature = "process", any(target_os = "linux", all(test, unix))))]
fn scope_create_refusal(mount: &std::path::Path, error: io::Error) -> io::Error {
    match error.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::ReadOnlyFilesystem => io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "lgwks_std::process (CgroupScope::create): no child cgroup is creatable under {} ({}), so this host has no cgroup containment",
                mount.display(),
                error.kind(),
            ),
        ),
        _ => error,
    }
}

#[cfg(all(not(unix), feature = "process"))]
/// Construct the unsupported-operation error for non-Unix targets, Windows
/// included: the stop observation and the release are Unix-only there too.
fn unsupported(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Seeded simulation tests for batch descendant capture (`capture_descendants_many`).
#[cfg(test)]
mod sim_capture_many;
/// Seeded simulation tests for the sweep that never stops at a refusal (#913).
#[cfg(test)]
mod sim_signal_every;
/// The module's unit tests, kept at `lgwks_std::process::tests`.
#[cfg(test)]
mod tests;

#[cfg(all(test, unix, feature = "process"))]
mod scope_refusal_tests {
    //! The cgroup scope `mkdir` refusal map, on every unix test host: a host
    //! fact falls back, a taken name stands (#913 item 8).

    use std::io::{Error, ErrorKind};
    use std::path::Path;

    use super::scope_create_refusal;

    /// The kind each `mkdir` failure becomes: the three host facts fall back,
    /// and every other kind, `AlreadyExists` first, reaches the caller as is.
    const CASES: [(ErrorKind, ErrorKind); 6] = [
        (ErrorKind::NotFound, ErrorKind::Unsupported),
        (ErrorKind::PermissionDenied, ErrorKind::Unsupported),
        (ErrorKind::ReadOnlyFilesystem, ErrorKind::Unsupported),
        (ErrorKind::AlreadyExists, ErrorKind::AlreadyExists),
        (ErrorKind::InvalidInput, ErrorKind::InvalidInput),
        (ErrorKind::Other, ErrorKind::Other),
    ];

    #[test]
    fn each_mkdir_failure_maps_to_its_one_answer() {
        let mount = Path::new("mount");
        for (raised, expected) in CASES {
            let answer = scope_create_refusal(mount, Error::from(raised));
            assert_eq!(
                answer.kind(),
                expected,
                "{raised:?} must become {expected:?}"
            );
        }
    }

    #[test]
    fn a_fallback_names_the_mount_it_could_not_use() {
        let answer = scope_create_refusal(
            Path::new("stand-in-mount"),
            Error::from(ErrorKind::NotFound),
        );
        assert!(
            answer.to_string().contains("stand-in-mount"),
            "the refusal names the mount: {answer}"
        );
    }
}
