//! The one error an approval test (#784) fails with, shared by the point tests
//! and the seeded families so a refusal reads the same in both.

use lgwks_bot::script::{DecisionError, FlowError};
use lgwks_bot::task::{HostError, StoreError};

/// What an approval test can fail with: every arm is a typed refusal from the
/// run path, so a failure names the fact that moved rather than a string.
#[derive(Debug)]
pub(crate) enum ApprovalError {
    /// The run failed.
    Flow(FlowError),
    /// The decision was refused.
    Decision(DecisionError),
    /// The store refused.
    Store(StoreError),
    /// The host was refused.
    Host(HostError),
    /// The device refused.
    Io(std::io::Error),
    /// The fixture could not be built, or a run ended other than the test
    /// required.
    Setup { what: &'static str },
}

impl std::fmt::Display for ApprovalError {
    /// The refusal behind the failure.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Flow(ref error) => write!(formatter, "the run failed: {error}"),
            Self::Decision(ref error) => write!(formatter, "the decision failed: {error}"),
            Self::Store(ref error) => write!(formatter, "the store failed: {error}"),
            Self::Host(ref error) => write!(formatter, "the host failed: {error}"),
            Self::Io(ref error) => write!(formatter, "the device failed: {error}"),
            Self::Setup { what } => write!(formatter, "the fixture failed: {what}"),
        }
    }
}

impl std::error::Error for ApprovalError {
    /// The refusal behind the failure, when it has one.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Flow(ref error) => Some(error),
            Self::Decision(ref error) => Some(error),
            Self::Store(ref error) => Some(error),
            Self::Host(ref error) => Some(error),
            Self::Io(ref error) => Some(error),
            Self::Setup { .. } => None,
        }
    }
}

impl From<FlowError> for ApprovalError {
    fn from(error: FlowError) -> Self {
        Self::Flow(error)
    }
}

impl From<DecisionError> for ApprovalError {
    fn from(error: DecisionError) -> Self {
        Self::Decision(error)
    }
}

impl From<StoreError> for ApprovalError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<HostError> for ApprovalError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

impl From<std::io::Error> for ApprovalError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
