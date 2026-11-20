//! The single-use faults a collection test injects into the subprocess, and the checks they share.
//!
//! The plan is a value handed to [`super::capture::run_bounded`], never a thread-local: two
//! collections running at once each see only their own armed faults and captures. A production
//! caller passes [`Faults::default`], which holds no plan, so every [`Faults::take`] answers `None`
//! without a lock or an allocation. Only a test can arm a plan ([`Faults::armed`] is `cfg(test)`).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// A step of the bounded subprocess where a test can inject one OS failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FaultPoint {
    /// Creating the stdout capture file.
    CaptureStdout,
    /// Creating the stderr capture file.
    CaptureStderr,
    /// Starting the child process.
    Spawn,
    /// Observing whether the child has exited.
    WaitObservation,
    /// Sampling the stdout capture's size.
    StatStdout,
    /// Sampling the stderr capture's size.
    StatStderr,
    /// Reading the stdout capture back.
    ReadStdout,
    /// Reading the stderr capture back.
    ReadStderr,
    /// Requesting the child's termination.
    Kill,
    /// Reaping the exited child.
    Reap,
    /// Removing a capture file.
    Unlink,
}

/// The faults still armed, in firing order, and every capture path the run created.
#[derive(Debug, Default)]
struct Plan {
    /// Faults not yet fired; only the front one can fire next.
    armed: VecDeque<FaultPoint>,
    /// Capture paths the run created, for the removal checks.
    captures: Vec<PathBuf>,
}

/// One collection's fault plan, shared by the run and any cleanup owner it hands back.
#[derive(Clone, Debug, Default)]
pub(super) struct Faults {
    /// `None` in production: no plan, no lock, every `take` answers `None`.
    plan: Option<Arc<Mutex<Plan>>>,
}

/// The plan behind the lock; a test that panicked while holding it left a plan that is still
/// the truth about which faults fired, so poisoning is not a reason to lose it.
fn hold(shared: &Mutex<Plan>) -> MutexGuard<'_, Plan> {
    match shared.lock() {
        Ok(plan) => plan,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Faults {
    /// A plan that fails each of `points` once, in order.
    #[cfg(test)]
    pub(super) fn armed(points: &[FaultPoint]) -> Self {
        let plan = Plan {
            armed: points.iter().copied().collect(),
            captures: Vec::new(),
        };
        Self {
            plan: Some(Arc::new(Mutex::new(plan))),
        }
    }

    /// The injected failure for `point` when it is the next armed fault, consuming it.
    pub(super) fn take(&self, point: FaultPoint) -> Option<std::io::Error> {
        let shared = self.plan.as_ref()?;
        let mut plan = hold(shared);
        if plan.armed.front() != Some(&point) {
            return None;
        }
        plan.armed.pop_front();
        Some(std::io::Error::other(format!(
            "injected metadata fault at {point:?}"
        )))
    }

    /// Records a capture path the run created, so a test can check it was removed.
    pub(super) fn record_capture(&self, path: &Path) {
        if let Some(shared) = self.plan.as_ref() {
            hold(shared).captures.push(path.to_path_buf());
        }
    }

    /// Whether every armed fault has fired; a plan with a fault left was never reached.
    #[cfg(test)]
    pub(super) fn is_spent(&self) -> bool {
        match self.plan.as_ref() {
            Some(shared) => hold(shared).armed.is_empty(),
            None => true,
        }
    }

    /// Every capture path recorded since the plan was armed or last read.
    #[cfg(test)]
    pub(super) fn captured_paths(&self) -> Vec<PathBuf> {
        match self.plan.as_ref() {
            Some(shared) => std::mem::take(&mut hold(shared).captures),
            None => Vec::new(),
        }
    }
}

/// Retries cleanup for the same bounded quanta `Drop` allows. A killed
/// child is not reaped the instant the signal lands, so one
/// `retry_cleanup` legitimately reports `false` on a loaded host.
#[cfg(test)]
pub(super) fn retry_until_resolved(
    obligation: &mut super::capture::CleanupObligation,
) -> std::io::Result<bool> {
    for _ in 0..super::capture::DROP_CLEANUP_QUANTA {
        if obligation.retry_cleanup()? {
            return Ok(true);
        }
        super::capture::poll_quantum();
    }
    obligation.retry_cleanup()
}

#[cfg(test)]
pub(super) fn assert_paths_removed(paths: &[PathBuf]) {
    for path in paths {
        assert_eq!(
            std::fs::metadata(path).err().map(|error| error.kind()),
            Some(std::io::ErrorKind::NotFound),
            "capture path should be gone: {}",
            path.display()
        );
    }
}

/// Test bodies propagate with `?` rather than panicking: `unwrap` is
/// forbidden workspace-wide, and a failing edge extraction should surface
/// as the error it is, not as a panic with no variant attached.
#[cfg(test)]
pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;
