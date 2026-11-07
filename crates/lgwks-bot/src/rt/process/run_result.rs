//! Supervised child process run results and captured streams.

use std::fmt;
use std::io;
use std::process::ExitStatus;

use super::frames::Frames;
use crate::rt::supervise::{CleanupReceipt, Containment};

/// The bytes retained from one captured standard stream, with its ceiling
/// accounting.
///
/// The head of the stream is what survives truncation: the first
/// [`StdioPolicy::Capture`](super::StdioPolicy::Capture) bytes, and no more. `total_bytes` is every byte the
/// child wrote, so `truncated` is decidable without retaining the tail, and a
/// reader that needs the exact size of a large output still has it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
#[doc(hidden)]
pub(crate) struct CapturedStream {
    /// The retained head of the stream, at most the capture ceiling.
    bytes: Vec<u8>,
    /// Every byte the child wrote to this stream.
    total_bytes: u64,
    /// Whether the child wrote more than the retained ceiling.
    truncated: bool,
}

impl CapturedStream {
    /// The retained bytes, at most the capture ceiling.
    #[must_use]
    #[doc(hidden)]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Every byte the child wrote, retained or not.
    #[must_use]
    pub(crate) const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Whether the child wrote more bytes than the retained ceiling.
    #[must_use]
    #[doc(hidden)]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// The stream as valid UTF-8, or `None` when it is not.
    ///
    /// Truncation happens at arbitrary byte boundaries and can bisect a
    /// multi-byte UTF-8 sequence, so a truncated stream that was valid text
    /// can report `None` here. See [`CapturedStream::lossy_text`].
    #[must_use]
    #[doc(hidden)]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// The stream decoded lossily, with malformed bytes replaced with the
    /// Unicode replacement character (`U+FFFD`).
    #[must_use]
    pub(crate) fn lossy_text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// True when the child wrote nothing to this stream.
    #[must_use]
    #[doc(hidden)]
    pub const fn is_empty(&self) -> bool {
        self.total_bytes == 0
    }

    /// How many bytes this capture is retaining.
    #[must_use]
    pub(crate) fn retained_capacity(&self) -> usize {
        self.bytes.len()
    }

    /// Decode the retained bytes as length-delimited binary frames (#685).
    ///
    /// The iterator yields one [`FrameRead`] per call up to `ceiling` bytes
    /// each. Truncation of the capture surfaces as a terminal
    /// [`FrameReadError::Truncated`]; a frame that exceeds `ceiling` is
    /// rejected with [`FrameReadError::Excessive`]. The iterator retains
    /// no stream-level allocations beyond its borrow of the capture.
    ///
    /// The parser validates frame boundaries and payload length delimiters,
    /// rejecting corrupt or malformed streams before returning. Empty frames
    /// are permitted. A frame boundary with no subsequent bytes yields
    /// [`Option::None`] cleanly.
    ///
    /// Note that `self.truncated()` indicates whether the capture itself
    /// was cut off by the supervisor's retention ceiling, not any
    /// truncation a child actually performed on its own last record.
    ///
    /// [`FrameReadError`]: crate::rt::process::FrameReadError
    #[must_use]
    pub(crate) fn frames(&self, ceiling: usize) -> Frames {
        Frames::of_capture(
            self.bytes.as_slice(),
            ceiling,
            self.truncated,
            self.retained_capacity(),
        )
    }

    /// Build one capture result. Crate-internal: the only producer is the
    /// supervisor's pipe drain.
    #[cfg(unix)]
    pub(crate) fn from_parts(bytes: Vec<u8>, total_bytes: u64, truncated: bool) -> Self {
        Self {
            bytes,
            total_bytes,
            truncated,
        }
    }
}

/// What a process run by
/// [`Supervisor::run_process`](crate::rt::supervise::Supervisor::run_process)
/// reported: how it ended, what it wrote, and how its process group was cleaned
/// up.
///
/// A successful function return is not a claim the command succeeded: the exit
/// status is data here and judging it is the caller's job. A deadline kill is
/// reported through [`ProcessRun::deadline_fired`] rather than as a normal
/// exit, so "the command finished" and "the supervisor stopped it" are never
/// the same report.
#[derive(Debug)]
#[non_exhaustive]
#[doc(hidden)]
pub struct ProcessRun {
    /// The status the engine reported, or `None` when the supervisor stopped
    /// the child (deadline) or could not read a status.
    pub(crate) status: Option<ExitStatus>,
    /// Whether this supervisor stopped the child because its deadline elapsed.
    pub(crate) deadline_fired: bool,
    /// Captured stdout, empty when the policy was not `Capture`.
    pub(crate) stdout: CapturedStream,
    /// Captured stderr, empty when the policy was not `Capture`.
    pub(crate) stderr: CapturedStream,
    /// Process-group cleanup evidence.
    pub(crate) cleanup: CleanupReceipt,
    /// What the cleanup read, signalled and could not account for.
    pub(crate) containment: Containment,
}

impl ProcessRun {
    /// The exit status the engine reported, when there is one.
    ///
    /// `None` means the child was stopped by the deadline or its status could
    /// not be read; it is not a success. On Unix a signal death reports
    /// [`ExitStatus::code`]`() == None` too, so use
    /// [`ProcessRun::signal`] to tell the two apart.
    #[must_use]
    #[doc(hidden)]
    pub const fn status(&self) -> Option<ExitStatus> {
        self.status
    }

    /// The code the child passed to `exit`, or `None` if it did not exit.
    ///
    /// `None` covers both a child still running and one killed by a signal, and
    /// the two are told apart by [`Self::signal`] on Unix or by
    /// [`Self::deadline_fired`]. A code of `0` is a normal successful exit and
    /// is not the same answer as `None`.
    #[must_use]
    pub(crate) fn exit_code(&self) -> Option<i32> {
        self.status.and_then(|status| status.code())
    }

    /// The terminating signal, when the child was killed by one (Unix).
    ///
    /// Distinct from an exit code: `kill -TERM $$` reports a signal and no
    /// code, while `exit 0` reports code 0 and no signal. Neither is a claim
    /// about the work the command was asked to do.
    #[cfg(unix)]
    #[must_use]
    pub(crate) fn signal(&self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        self.status.and_then(|status| status.signal())
    }

    /// Whether the supervisor stopped the child because its deadline elapsed.
    ///
    /// A deadline kill reaps the whole process group (INV-BOT-9), so this is a
    /// stop the supervisor ordered, not an exit the command chose.
    #[must_use]
    pub(crate) const fn deadline_fired(&self) -> bool {
        self.deadline_fired
    }

    /// What the child wrote to standard output, up to the capture ceiling.
    ///
    /// Empty unless [`ProcessSpec::stdout_policy`](super::ProcessSpec::stdout_policy) was `Capture`. The retained bytes
    /// are the *head* of the stream, so [`CapturedStream::truncated`] is how a
    /// caller knows the tail is missing; [`CapturedStream::total_bytes`] says
    /// how much there would have been.
    #[must_use]
    #[doc(hidden)]
    pub const fn stdout(&self) -> &CapturedStream {
        &self.stdout
    }

    /// What the child wrote to standard error, up to the capture ceiling.
    ///
    /// The same shape as [`Self::stdout`], and empty unless
    /// [`ProcessSpec::stderr_policy`](super::ProcessSpec::stderr_policy) was `Capture`. A child that died before writing
    /// leaves an empty capture rather than none, so emptiness is never evidence
    /// that the policy was not `Capture`.
    #[must_use]
    #[doc(hidden)]
    pub const fn stderr(&self) -> &CapturedStream {
        &self.stderr
    }

    /// Process-group cleanup evidence.
    ///
    /// A borrow, where this returned the receipt by value: the receipt carries the
    /// pids it could not prove gone, so a by-value read would copy that list per
    /// call.
    #[must_use]
    #[doc(hidden)]
    pub const fn cleanup(&self) -> &CleanupReceipt {
        &self.cleanup
    }

    /// What the cleanup read, signalled, and could not account for.
    ///
    /// The receipt says whether the tree was stopped; this says how much was
    /// looked at to say it — which mechanism read the process table, how many
    /// descendants it named, how many a signal reached, which of them were still
    /// running when the drain ended, and which limit of the mechanism produced
    /// that. A caller that has to report or re-drive the cleanup reads this.
    #[must_use]
    #[doc(hidden)]
    pub const fn containment(&self) -> &Containment {
        &self.containment
    }
}

/// Why [`Supervisor::run_process`](crate::rt::supervise::Supervisor::run_process)
/// could not produce a [`ProcessRun`].
///
/// The vocabulary draws the one distinction a caller's next move turns on:
/// whether the program ever ran. [`ProcessRunError::Refused`] and
/// [`ProcessRunError::NotStarted`] establish that nothing ran, so a retry is
/// safe; [`ProcessRunError::AfterStart`] establishes that the child was
/// started and its outcome is unknown, so a retry is a possible duplicate.
#[derive(Debug)]
#[non_exhaustive]
#[doc(hidden)]
pub enum ProcessRunError {
    /// The supervisor refused to start anything: it was cancelled before the
    /// fork, so no instruction of the program ran.
    Refused,
    /// The platform refused to start the program at all — it does not exist,
    /// is not executable, or the fork was refused. Nothing ran.
    NotStarted {
        /// The platform's refusal.
        source: io::Error,
    },
    /// The process started and was then stopped by this supervisor before it
    /// exited on its own, or its terminal status could not be read. It ran, so
    /// the effect is indeterminate.
    AfterStart {
        /// Why the run could not be settled.
        source: io::Error,
    },
}

impl fmt::Display for ProcessRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Refused => formatter.write_str("the supervisor refused to start a process"),
            Self::NotStarted { ref source } => {
                write!(formatter, "the program did not start: {source}")
            }
            Self::AfterStart { ref source } => {
                write!(
                    formatter,
                    "the process started but did not settle: {source}"
                )
            }
        }
    }
}

impl std::error::Error for ProcessRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Refused => None,
            Self::NotStarted { ref source } | Self::AfterStart { ref source } => Some(source),
        }
    }
}
