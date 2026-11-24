//! The bounded `cargo` subprocess: capture files, the deadline, the byte budgets and the cleanup obligation.

use std::ffi::OsString;
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::MetadataError;
use super::faults::{FaultPoint, Faults};

/// Elapsed-deadline budget for the Cargo subprocess: `cargo metadata` on
/// this workspace answers in about a second, so a healthy run never notices
/// the 120s collection ceiling. The ceiling bounds collection polling; OS
/// termination and reap failures are reported separately (issue #159).
pub(super) const METADATA_TIMEOUT: Duration = Duration::from_secs(120);
/// Per-stream retained-byte budget for the Cargo subprocess. The 5ms polling
/// interval makes disk growth between samples possible, so this is not a hard
/// on-disk quota (issue #159 M3).
pub(super) const METADATA_STREAM_CAP: usize = 8 * 1024 * 1024;
/// Exclusive-create attempts per capture file: each attempt draws a fresh
/// distinguisher, so this bound is never reached by chance, only by a
/// broken clock or a hostile temp dir, both of which refuse (issue #143 R14).
const CAPTURE_ATTEMPTS: usize = 16;

/// A reaped child's exit status with its bounded output.
pub(super) struct BoundedOutput {
    /// What the child exited with; a non-zero status is Cargo's refusal.
    pub(super) status: std::process::ExitStatus,
    /// At most the per-stream budget of stdout bytes.
    pub(super) stdout: Vec<u8>,
    /// At most the per-stream budget of stderr bytes.
    pub(super) stderr: Vec<u8>,
    /// A capture-removal failure after complete output, reported beside it.
    pub(super) unresolved: Option<Unresolved>,
}

/// Capture removals that failed after a complete collection, and the owner
/// that can retry them.
#[derive(Debug)]
pub(super) struct Unresolved {
    /// Every removal failure, in the order of the obligation's report.
    pub(super) captures: Vec<std::io::Error>,
    /// The owner of the paths that could not be removed.
    pub(super) obligation: Box<CleanupObligation>,
}

impl Unresolved {
    /// The cleanup failure as a refusal, with `cause` as its primary failure
    /// when collection was refused afterwards.
    pub(super) fn into_error(self, cause: Option<MetadataError>) -> MetadataError {
        MetadataError::ProcessCleanup {
            cause: cause.map(Box::new),
            process: None,
            captures: self.captures,
            obligation: self.obligation,
        }
    }
}

/// Owns capture pathnames from the instant a capture file is created through
/// every later fallible collection step.
#[derive(Debug)]
pub(super) struct CaptureFiles {
    /// Paths whose names this call created exclusively.
    pub(super) paths: Vec<PathBuf>,
    /// The run's fault plan; empty outside a fault-injection test.
    faults: Faults,
}

/// A refusal on its way back to the caller, recorded where it is decided.
fn refuse<T, E: std::fmt::Debug>(error: E, site: &'static str) -> Result<T, E> {
    lgwks_std::trace::debug!(error = ?error, site, "capture: returning an error to the caller");
    Err(error)
}

/// Calls the operating system unless the run's plan arms a fault here.
fn remove_capture(path: &Path, faults: &Faults) -> std::io::Result<()> {
    if let Some(error) = faults.take(FaultPoint::Unlink) {
        return refuse(error, "remove_capture");
    }
    std::fs::remove_file(path)
}

/// Observes the direct child without treating an observation error as absence.
fn observe_child(
    child: &mut std::process::Child,
    faults: &Faults,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    if let Some(error) = faults.take(FaultPoint::WaitObservation) {
        return refuse(error, "observe_child");
    }
    child.try_wait()
}

/// Requests termination and preserves an OS refusal for the cleanup owner.
///
/// On Unix the child leads a group of its own, and the whole group is killed
/// first: a hung Cargo's own children (`rustc`, `git`) end with it rather than
/// outliving the deadline (#1128 G2b). The child's own kill is the result.
fn terminate_child(child: &mut std::process::Child, faults: &Faults) -> std::io::Result<()> {
    if let Some(error) = faults.take(FaultPoint::Kill) {
        return refuse(error, "terminate_child");
    }
    #[cfg(unix)]
    match i32::try_from(child.id()) {
        Ok(pgid) => {
            if let Err(error) = lgwks_std::process::kill_process_group(pgid) {
                lgwks_std::trace::debug!(error = ?error, pgid, "terminate_child: the group was already gone");
            }
        }
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "terminate_child: the pid is no group id");
        }
    }
    child.kill()
}

/// Makes the child lead a process group of its own, so [`terminate_child`]
/// reaches everything it started.
#[cfg(unix)]
fn lead_group(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0)
}

/// Makes the child lead a process group of its own, where the platform has groups.
#[cfg(not(unix))]
fn lead_group(command: &mut Command) -> &mut Command {
    command
}

/// Waits for a direct child; an error leaves it in the retryable owner.
fn reap_child(
    child: &mut std::process::Child,
    faults: &Faults,
) -> std::io::Result<std::process::ExitStatus> {
    if let Some(error) = faults.take(FaultPoint::Reap) {
        return refuse(error, "reap_child");
    }
    child.wait()
}

impl CaptureFiles {
    /// Starts an empty cleanup owner before acquiring the first file.
    pub(super) fn new(faults: Faults) -> Self {
        Self {
            paths: Vec::new(),
            faults,
        }
    }

    /// Records a newly acquired path before another operation can fail.
    pub(super) fn own(&mut self, path: PathBuf) {
        self.faults.record_capture(&path);
        self.paths.push(path);
    }

    /// Removes every owned path and returns every failure except confirmed
    /// absence. Failed removals remain visible to the caller.
    pub(super) fn cleanup(&mut self) -> Vec<std::io::Error> {
        let mut failures = Vec::new();
        let mut pending = Vec::new();
        for path in self.paths.drain(..) {
            match remove_capture(&path, &self.faults) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    failures.push(error);
                    pending.push(path);
                }
            }
        }
        self.paths = pending;
        failures
    }
}

impl Drop for CaptureFiles {
    /// Retries cleanup on early return; this fallback never reports success,
    /// so each removal it could not make is recorded where an operator sees it.
    fn drop(&mut self) {
        for failure in self.cleanup() {
            lgwks_std::trace::debug!(error = ?failure, "CaptureFiles::drop: a capture path could not be removed");
        }
    }
}

/// Retains ownership of a direct child or capture whose cleanup was not
/// confirmed. Keep this value and retry until it reports `true`.
#[must_use = "dropping an unresolved cleanup obligation abandons further observation"]
#[derive(Debug)]
pub struct CleanupObligation {
    /// The child whose termination or reap remains unresolved.
    child: Option<std::process::Child>,
    /// Capture paths whose removal remains unresolved.
    pub(super) captures: CaptureFiles,
    /// What failed when the obligation was created, fixed so the refusal's
    /// text does not change as retries make progress.
    pub(super) report: CleanupReport,
}

/// The facts a cleanup refusal names: which step failed, on which process, and
/// which capture paths could not be removed.
#[derive(Debug)]
pub(super) struct CleanupReport {
    /// The process step that failed, if one did.
    pub(super) step: Option<CleanupStep>,
    /// The direct child's process id, while the obligation still owns it.
    pub(super) pid: Option<u32>,
    /// Capture paths whose removal failed, in the order of the retained errors.
    pub(super) capture_paths: Vec<PathBuf>,
}

/// A process-cleanup step that can fail and leave the child owned.
#[derive(Debug, Clone, Copy)]
pub(super) enum CleanupStep {
    /// Sending the kill request.
    Kill,
    /// Waiting for the exited child to be reaped.
    Reap,
}

impl CleanupStep {
    /// The verb a refusal uses for this step.
    pub(super) fn verb(self) -> &'static str {
        match self {
            Self::Kill => "kill",
            Self::Reap => "reap",
        }
    }
}

impl CleanupObligation {
    /// Takes ownership of an unresolved child and capture set, recording what
    /// failed at this moment for the refusal's text.
    pub(super) fn new(
        step: Option<CleanupStep>,
        child: Option<std::process::Child>,
        captures: &mut CaptureFiles,
    ) -> Self {
        let paths = std::mem::take(&mut captures.paths);
        Self {
            report: CleanupReport {
                step,
                pid: child.as_ref().map(std::process::Child::id),
                capture_paths: paths.clone(),
            },
            child,
            captures: CaptureFiles {
                paths,
                faults: captures.faults.clone(),
            },
        }
    }

    /// A caller holding a [`MetadataError::ProcessCleanup`] calls this on its `obligation`
    /// until it answers `true`; the unit tests `cleanup_obligation_confirms_an_already_exited_child`
    /// and `injected_collection_fault_matrix_keeps_cleanup_owned_and_truthful` run that retry.
    pub fn retry_cleanup(&mut self) -> Result<bool, std::io::Error> {
        let faults = self.captures.faults.clone();
        if let Some(child) = self.child.as_mut() {
            if let Err(kill_error) = terminate_child(child, &faults) {
                match child.try_wait() {
                    Ok(Some(_)) => self.child = None,
                    Ok(None) => return refuse(kill_error, "retry_cleanup"),
                    Err(observation_error) => return refuse(observation_error, "retry_cleanup"),
                }
            }
            if let Some(child) = self.child.as_mut() {
                match observe_child(child, &faults)? {
                    Some(_) => self.child = None,
                    None => return Ok(false),
                }
            }
        }
        if let Some(error) = self.captures.cleanup().into_iter().next() {
            return refuse(error, "retry_cleanup");
        }
        Ok(self.captures.paths.is_empty())
    }
}

/// How many poll quanta a dropped obligation waits for its child to exit.
///
/// Twenty 5 ms quanta: long enough for a killed child to be reaped on a loaded
/// host, short enough that dropping an error never stalls the caller for more
/// than a tenth of a second.
pub(super) const DROP_CLEANUP_QUANTA: usize = 20;

/// A dropped obligation makes a last, bounded attempt to kill and reap.
///
/// The shipped gate reports a `ProcessCleanup` refusal as text and drops the
/// error, so without this the Cargo child it names was never killed or reaped
/// by anyone. A killed child is not reaped the instant the signal is sent, so
/// one `try_wait` would usually leave a zombie; this repeats the same `kill`
/// plus `try_wait` a retry makes for at most twenty poll quanta and then gives
/// up, because a destructor must not block without bound. Capture files have
/// their own `Drop`.
impl Drop for CleanupObligation {
    fn drop(&mut self) {
        for _ in 0..DROP_CLEANUP_QUANTA {
            match self.retry_cleanup() {
                Ok(true) => return,
                Ok(false) => {}
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "CleanupObligation::drop: a cleanup attempt failed; retrying");
                }
            }
            poll_quantum();
        }
    }
}

/// The largest capture `Take` can be asked to count, and the ceiling a declared
/// cap above it becomes.
///
/// `Take` counts in `u64` and a cap is a `usize`; a cap past `u64::MAX` is one no
/// platform can measure, so the ceiling is the only bound that is true of it.
/// The bound is named because two call sites needed it, and a ceiling written
/// twice is two ceilings.
const MAX_CAPTURE_BYTES: u64 = u64::MAX;

/// The byte ceiling `read_capped` enforces, in the width `Take` counts in.
fn capture_limit(cap: usize) -> u64 {
    match u64::try_from(cap) {
        Ok(limit) => limit,
        Err(_wider_than_u64) => MAX_CAPTURE_BYTES,
    }
}

/// One deadline-poll quantum.
///
/// A timed condvar wait rather than `sleep`: this crate is synchronous, so there
/// is no reactor to block, and a condvar wait is a bounded wait on an event
/// source. The condvar and its mutex are local to the call and nothing notifies
/// them, so the wait always ends by its timeout; the 5 ms quantum bounds
/// deadline overshoot and the poll loop is the only waiter, so it always stops.
pub(super) fn poll_quantum() {
    let quiet = std::sync::Mutex::new(());
    let wake = std::sync::Condvar::new();
    let held = match quiet.lock() {
        Ok(held) => held,
        Err(poisoned) => poisoned.into_inner(),
    };
    // Nothing else holds `quiet`, so a poisoned wake is the same timed-out
    // wake; either way the guard is released when this quantum ends.
    let _resumed = match wake.wait_timeout(held, POLL_QUANTUM) {
        Ok(woken) => woken,
        Err(poisoned) => poisoned.into_inner(),
    };
}

/// The length of one deadline-poll quantum.
const POLL_QUANTUM: Duration = Duration::from_millis(5);

/// The capture stream a step reads or measures.
#[derive(Clone, Copy)]
enum Stream {
    /// The child's standard output.
    Stdout,
    /// The child's standard error.
    Stderr,
}

impl Stream {
    /// The stream's name in a refusal.
    fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    /// The fault point at which this stream's capture file is created.
    fn capture_fault(self) -> FaultPoint {
        match self {
            Self::Stdout => FaultPoint::CaptureStdout,
            Self::Stderr => FaultPoint::CaptureStderr,
        }
    }

    /// The fault point at which this stream's size is sampled.
    fn stat_fault(self) -> FaultPoint {
        match self {
            Self::Stdout => FaultPoint::StatStdout,
            Self::Stderr => FaultPoint::StatStderr,
        }
    }

    /// The fault point at which this stream's capture is read back.
    fn read_fault(self) -> FaultPoint {
        match self {
            Self::Stdout => FaultPoint::ReadStdout,
            Self::Stderr => FaultPoint::ReadStderr,
        }
    }
}

/// One capture file: created empty and exclusive, unlinked on every
/// return path. Creation uses `create_new`, so even a distinguisher
/// collision is a retried name, never another call's file truncated.
fn capture(stream: Stream, faults: &Faults) -> Result<(PathBuf, File), MetadataError> {
    use std::io::ErrorKind;
    if let Some(error) = faults.take(stream.capture_fault()) {
        return refuse(MetadataError::Spawn(error), "capture");
    }
    // Bounded: each attempt draws a fresh distinguisher, so exhaustion
    // is a refusal shape, not a spin.
    for _ in 0..CAPTURE_ATTEMPTS {
        let path = std::env::temp_dir().join(format!(
            "lgwks-deps-{name}-{distinguisher}.capture",
            name = stream.name(),
            distinguisher = distinguisher()?,
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return refuse(MetadataError::Spawn(error), "capture"),
        }
    }
    Err(MetadataError::Spawn(std::io::Error::from(
        ErrorKind::AlreadyExists,
    )))
}

/// Names one capture file: 128 bits of OS entropy (`lgwks_std::random`,
/// the one source INV-RANDOM-1 allows). A process id or a clock
/// would be reused by the OS and could name another call's file. An
/// entropy refusal names no file and starts no child; the call ends here.
#[cfg(not(target_family = "wasm"))]
fn distinguisher() -> Result<String, MetadataError> {
    lgwks_std::random::bytes::<16>()
        .map(lgwks_std::hex::encode)
        .map_err(MetadataError::Entropy)
}

/// wasm has no entropy source: `lgwks_std::random` refuses the target
/// with a `compile_error!`, and the WASI boundary job builds this crate
/// for `wasm32-wasip1`. Nanos plus a monotone sequence name the file,
/// and `create_new` above is what makes that safe: a repeated name is
/// retried, never opened. Spawning cannot succeed on this target anyway,
/// so these files only ever live until the `Spawn` refusal unlinks them.
#[cfg(target_family = "wasm")]
fn distinguisher() -> Result<String, MetadataError> {
    static CAPTURE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // A clock that reads before the Unix epoch has no nanos to offer, and
    // flooring it to the epoch would name every capture of this process the
    // same instant. The sequence alone cannot separate two runs, so the
    // refusal is the honest answer: a name this process cannot distinguish is
    // not a distinguishable name.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|before_epoch| {
            MetadataError::Spawn(std::io::Error::other(format!(
                "the wall clock reads before the Unix epoch: {before_epoch}"
            )))
        })?
        .as_nanos();
    let seq = CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(format!("{nanos}-{seq}"))
}

/// Read at most one byte past budget: anything longer is an overflow,
/// and the retained bytes are dropped with the Vec, never decoded.
fn read_capped(
    path: &Path,
    stream_cap: usize,
    stream: Stream,
    faults: &Faults,
) -> Result<(Vec<u8>, bool), MetadataError> {
    if let Some(error) = faults.take(stream.read_fault()) {
        return refuse(MetadataError::Spawn(error), "read_capped");
    }
    let limit = capture_limit(stream_cap);
    let file = File::open(path).map_err(MetadataError::Spawn)?;
    let mut kept = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut kept)
        .map_err(MetadataError::Spawn)?;
    let overflow = kept.len() > stream_cap;
    Ok((kept, overflow))
}

/// Keeps the original failure distinct from any cleanup failure.
fn cleanup_error(
    cause: Option<MetadataError>,
    process: Option<(CleanupStep, std::io::Error)>,
    child: Option<std::process::Child>,
    captures: &mut CaptureFiles,
) -> MetadataError {
    let capture_errors = captures.cleanup();
    if process.is_none()
        && capture_errors.is_empty()
        && let Some(cause) = cause
    {
        return cause;
    }
    let (step, process) = match process {
        Some((step, error)) => (Some(step), Some(error)),
        None => (None, None),
    };
    MetadataError::ProcessCleanup {
        cause: cause.map(Box::new),
        process,
        captures: capture_errors,
        obligation: Box::new(CleanupObligation::new(step, child, captures)),
    }
}

/// A refusal before any child exists: only the captures need cleaning.
fn refuse_before_child<T>(
    cause: MetadataError,
    captures: &mut CaptureFiles,
) -> Result<T, MetadataError> {
    refuse(
        cleanup_error(Some(cause), None, None, captures),
        "run_bounded",
    )
}

/// Creates one capture and hands its path to the cleanup owner at once.
fn owned_capture(
    stream: Stream,
    captures: &mut CaptureFiles,
) -> Result<(PathBuf, File), MetadataError> {
    let faults = captures.faults.clone();
    match capture(stream, &faults) {
        Ok((path, file)) => {
            captures.own(path.clone());
            Ok((path, file))
        }
        Err(error) => refuse_before_child(error, captures),
    }
}

/// `Ok(true)` when `stream`'s capture is past budget at this sample.
fn past_budget(
    path: &Path,
    stream: Stream,
    stream_cap: usize,
    faults: &Faults,
) -> Result<bool, MetadataError> {
    if let Some(error) = faults.take(stream.stat_fault()) {
        let refusal = Err(MetadataError::Spawn(error));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "past_budget: returning an error to the caller");
        return refusal;
    }
    let size = std::fs::metadata(path).map_err(MetadataError::Spawn)?;
    Ok(size.len() > capture_limit(stream_cap))
}

/// The two capture paths a running child writes to.
struct Paths<'paths> {
    /// Where the child's stdout lands.
    stdout: &'paths Path,
    /// Where the child's stderr lands.
    stderr: &'paths Path,
}

/// Polls the child until it exits, the deadline passes, or a capture runs
/// past budget. The only waiter, and it always stops: each quantum either
/// observes the exit, fires the deadline, or finds a capture file past
/// budget. File sizes only grow while the child lives, so a stat past budget
/// stands against every later one.
fn poll_until_exit(
    child: &mut std::process::Child,
    paths: &Paths<'_>,
    timeout: Duration,
    stream_cap: usize,
    faults: &Faults,
) -> Result<std::process::ExitStatus, MetadataError> {
    // A deadline the monotonic clock cannot represent is not a deadline: the
    // clock's range is finite and a `timeout` past its end never arrives, so the
    // byte budget is what bounds the child. Collapsing it to `now` instead would
    // refuse every child immediately over a timeout of a few centuries.
    let deadline = std::time::Instant::now().checked_add(timeout);
    loop {
        if let Some(status) = observe_child(child, faults).map_err(MetadataError::Spawn)? {
            return Ok(status);
        }
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            let refusal = Err(MetadataError::Timeout { after: timeout });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "poll_until_exit: returning an error to the caller");
            return refusal;
        }
        for (path, stream) in [
            (paths.stdout, Stream::Stdout),
            (paths.stderr, Stream::Stderr),
        ] {
            if past_budget(path, stream, stream_cap, faults)? {
                let refusal = Err(MetadataError::OutputTooLarge {
                    stream: stream.name(),
                    limit: stream_cap,
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "poll_until_exit: returning an error to the caller");
                return refusal;
            }
        }
        poll_quantum();
    }
}

/// Kills and reaps a child after `cause` refused the run, keeping a failed
/// step and the child in the obligation the refusal carries.
fn stop_after_refusal(
    cause: MetadataError,
    mut child: std::process::Child,
    captures: &mut CaptureFiles,
) -> MetadataError {
    let faults = captures.faults.clone();
    let process_error = match terminate_child(&mut child, &faults) {
        Ok(()) => match reap_child(&mut child, &faults) {
            Ok(_) => None,
            Err(error) => Some((CleanupStep::Reap, error)),
        },
        Err(kill_error) => match child.try_wait() {
            Ok(Some(_)) => None,
            Ok(None) => Some((CleanupStep::Kill, kill_error)),
            Err(observation) => {
                lgwks_std::trace::debug!(error = ?observation, "stop_after_refusal: the child could not be observed after a failed kill");
                Some((CleanupStep::Kill, kill_error))
            }
        },
    };
    match process_error {
        Some(error) => cleanup_error(Some(cause), Some(error), Some(child), captures),
        None => cleanup_error(Some(cause), None, None, captures),
    }
}

/// Runs one subprocess to completion under a deadline and output budgets.
///
/// The child writes to two capture files, never to pipes: a flood cannot
/// block the child on a full pipe, and no reader thread can be held open by
/// a grandchild that inherited a descriptor. Each poll quantum stats both
/// files, and the first one past budget requests direct-child termination
/// before returning [`MetadataError::OutputTooLarge`]; the deadline does the
/// same with [`MetadataError::Timeout`]. Termination, reap and capture-removal
/// failures are retained in [`MetadataError::ProcessCleanup`]. A refusal
/// carries no partial graph: the gate reports the failure rather than
/// decoding a prefix of it.
///
/// On Unix the kill reaches the child's whole process group. A descendant that
/// left the group, or one alive after the direct child exited on its own, may
/// keep an inherited capture descriptor and go on writing; the sampled
/// retained-byte cap does not bound that writer's disk use. The supported
/// profile is trusted Cargo and its group, not hostile descendant containment.
///
/// `faults` is [`Faults::default`] outside a fault-injection test.
pub(super) fn run_bounded(
    program: &str,
    args: &[OsString],
    dir: &Path,
    timeout: Duration,
    stream_cap: usize,
    faults: &Faults,
) -> Result<BoundedOutput, MetadataError> {
    let mut captures = CaptureFiles::new(faults.clone());
    let (stdout_path, stdout_file) = owned_capture(Stream::Stdout, &mut captures)?;
    let (stderr_path, stderr_file) = owned_capture(Stream::Stderr, &mut captures)?;
    let spawned = match faults.take(FaultPoint::Spawn) {
        Some(error) => Err(error),
        None => lead_group(&mut Command::new(program))
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .spawn(),
    };
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return refuse_before_child(MetadataError::Spawn(error), &mut captures),
    };
    let paths = Paths {
        stdout: &stdout_path,
        stderr: &stderr_path,
    };
    let status = match poll_until_exit(&mut child, &paths, timeout, stream_cap, faults) {
        Err(cause) => {
            let refusal = Err(stop_after_refusal(cause, child, &mut captures));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "run_bounded: returning an error to the caller");
            return refusal;
        }
        Ok(status) => match reap_child(&mut child, faults) {
            Ok(_) => status,
            Err(error) => {
                let cleanup = cleanup_error(
                    None,
                    Some((CleanupStep::Reap, error)),
                    Some(child),
                    &mut captures,
                );
                return refuse(cleanup, "run_bounded");
            }
        },
    };
    let mut streams = Vec::with_capacity(2);
    for (path, stream) in [
        (&stdout_path, Stream::Stdout),
        (&stderr_path, Stream::Stderr),
    ] {
        match read_capped(path, stream_cap, stream, faults) {
            Ok((bytes, false)) => streams.push(bytes),
            Ok((_, true)) => {
                let cause = MetadataError::OutputTooLarge {
                    stream: stream.name(),
                    limit: stream_cap,
                };
                return refuse_before_child(cause, &mut captures);
            }
            Err(error) => return refuse_before_child(error, &mut captures),
        }
    }
    let mut streams = streams.into_iter();
    let (Some(stdout), Some(stderr)) = (streams.next(), streams.next()) else {
        return refuse_before_child(
            MetadataError::Spawn(std::io::Error::other("a capture stream was not read")),
            &mut captures,
        );
    };
    // Both streams are read in full and the child is reaped, so the output is
    // complete. A capture that cannot be removed now is reported beside it,
    // not in place of it: the graph is valid, and the leftover path is owned
    // by the obligation for a retry.
    let cleanup_errors = captures.cleanup();
    let unresolved = (!cleanup_errors.is_empty()).then(|| Unresolved {
        captures: cleanup_errors,
        obligation: Box::new(CleanupObligation::new(None, None, &mut captures)),
    });
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
        unresolved,
    })
}
