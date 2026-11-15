//! Black-box acceptance for `Supervisor::run_process` and the `domain::sys`
//! process binding.
//!
//! `run_process` is the result-bearing counterpart of `spawn_process`: it runs
//! one child to completion under the same in-flight ceiling, process-group
//! ownership and deadline machinery, and reports what it observed. These tests
//! pin the properties that make its report usable:
//!
//! - the exit code and the signal are reported distinctly, and neither is a
//!   claim about the work the command was asked to do (T35);
//! - stdout and stderr are captured exactly up to their ceiling, a saturating
//!   child is drained and truncated rather than blocked or retained without
//!   bound, and the exact total is still reported (T05);
//! - a deadline stops the whole group and is reported as a deadline, not as a
//!   normal exit;
//! - a parent's zero exit does not fabricate tree cleanup: the descendant is
//!   gone when the report says cleanup is confirmed (T19);
//! - dropping the run future after the fork leaves no orphan (T20);
//! - the sanctioned path stays bounded under many concurrent processes;
//! - a refusal before the fork and a failure after it are different types.
//!
//! The `domain::sys::Process` half drives the same machinery through the verb
//! traits and checks the dispatch certainty it reports.
//!
//! The rows live beside this file by subject: `run` (the supervisor's own run
//! report), `frames` (the length-prefixed grammar over a captured stream) and
//! `verb` (the `domain::sys::Process` verbs, including their framed door).
#![cfg(all(
    unix,
    feature = "rt",
    feature = "time",
    feature = "sync",
    feature = "process"
))]

use std::num::NonZeroUsize;

use lgwks_bot::rt::process::ProcessSpec;

mod frames;
mod run;
mod verb;

/// What a test reports when a precondition did not hold.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A shell spec running `script`, with stderr on stdout's stream when asked.
fn shell(script: &str) -> ProcessSpec {
    let mut spec = ProcessSpec::new("sh");
    spec.arg("-c").arg(script);
    spec
}

/// A shell spec with the given capture ceilings and no deadline.
fn captured_shell(script: &str, limit: usize) -> ProcessSpec {
    let mut spec = shell(script);
    // A zero limit captures nothing, so the streams are left as they are rather
    // than captured at a one-byte ceiling nobody asked for.
    if let Some(limit) = NonZeroUsize::new(limit) {
        spec.capture_stdout(limit);
        spec.capture_stderr(limit);
    }
    spec
}
