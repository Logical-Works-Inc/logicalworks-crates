//! Run disposition, external effect knowledge, and terminal record mapping.

use std::fmt;
use std::sync::OnceLock;

use crate::script::{Durable, FlowError};

use super::Report;
use super::request::{RequestError, TerminalRecord};

/// The failure a report names if it carries neither an output nor an error.
pub(crate) fn own_fault() -> &'static FlowError {
    OWN_FAULT
        .get_or_init(|| FlowError::failed("a report with neither an output nor a located failure"))
}

/// The storage behind [`own_fault`].
static OWN_FAULT: OnceLock<FlowError> = OnceLock::new();

/// How a run ended.
///
/// The disposition is the run's own terminal state; it is never inferred from
/// the output's shape, and it survives a step trail that overflowed.
///
/// # Examples
///
/// ```
/// lgwks_bot::script! {
///     flow check_disp():
///         give back lgwks_bot::task::Disposition::Succeeded.label()
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Disposition {
    /// The body returned its value. It performs no journaled effect, so this
    /// says nothing about the state of anything outside the process.
    Succeeded,
    /// The body returned an error that was neither a stop nor a timeout.
    Failed,
    /// The run was admitted and then stopped: the host was cancelled, or the
    /// body cancelled its own scope.
    Cancelled,
    /// The run's deadline expired, either while it waited for a permit or
    /// while its body was running.
    DeadlineExceeded,
    /// The run was never admitted. The host was already cancelled, or was
    /// cancelled while the run waited for a permit, so no body ever ran.
    Refused,
    /// The run was admitted against the host's budget but refused before its body
    /// started: this host's grants do not cover what the task declared it needs.
    Blocked,
    /// The run parked awaiting a human decision and holds no task.
    Parked,
}

impl Disposition {
    /// The name of this disposition, for logs and reports.
    ///
    /// # Examples
    ///
    /// ```
    /// lgwks_bot::script! {
    ///     flow check_label():
    ///         give back lgwks_bot::task::Disposition::Cancelled.label()
    /// }
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::DeadlineExceeded => "DeadlineExceeded",
            Self::Refused => "Refused",
            Self::Blocked => "Blocked",
            Self::Parked => "Parked",
        }
    }

    /// Whether the run's body ran to a value.
    ///
    /// # Examples
    ///
    /// ```
    /// lgwks_bot::script! {
    ///     flow check_success():
    ///         give back lgwks_bot::task::Disposition::Succeeded.is_success()
    /// }
    /// ```
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

impl fmt::Display for Disposition {
    /// The [`Disposition::label`].
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// What a report knows about effects outside this process.
///
/// # Examples
///
/// ```
/// lgwks_bot::script! {
///     flow check_effects():
///         give back lgwks_bot::task::EffectKnowledge::None
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EffectKnowledge {
    /// No external effect is claimed.
    None,
    /// The run's steps that called [`remember`](crate::script::remember) are
    /// recorded durably under the run's [`RunId`](crate::effect::RunId).
    StepRecords {
        /// How many records this run committed over its whole life.
        records: usize,
    },
    /// As [`EffectKnowledge::StepRecords`], under a store that could not count
    /// the run's records when asked.
    StepRecordsUncounted,
}

/// The terminal record a submission of a request running under `report` writes.
pub(crate) fn terminal_for<O: Durable>(
    report: &Report<O>,
) -> Result<Option<TerminalRecord>, RequestError> {
    Ok(match report.disposition() {
        Disposition::Succeeded => {
            let output = report
                .output()
                .ok_or(RequestError::Record(FlowError::failed(
                    "a successful run reported no output to record",
                )))?;
            Some(TerminalRecord::succeeded(output)?)
        }
        Disposition::Failed | Disposition::DeadlineExceeded => {
            let error = report
                .error()
                .ok_or(RequestError::Record(FlowError::failed(
                    "a stopped run reported no error to record",
                )))?;
            Some(TerminalRecord::stopped(
                report.disposition(),
                error.at(),
                error.to_string(),
            ))
        }
        Disposition::Parked => None,
        Disposition::Cancelled | Disposition::Refused | Disposition::Blocked => None,
    })
}
