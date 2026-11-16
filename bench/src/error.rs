//! The rig's one error: every way a measurement, a probe or the report can fail.
//!
//! One variant per real failure, so a caller (and the process's exit message)
//! reads which refusal it was without parsing prose. The wrapped errors keep
//! their own type and are reported through `source`, and the three refusals the
//! rig raises itself keep the exact sentence a reader of the report has always
//! seen.

use std::fmt;
use std::io;
use std::path::PathBuf;

use lgwks_bot::broker::BrokerError;
use lgwks_bot::effect::{DigestError, IdError};
use lgwks_bot::app_error::BotError;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::KernelError;
use lgwks_std::trace::DebugInstallError;

/// Why the rig stopped.
#[derive(Debug)]
pub(crate) enum BenchError {
    /// The bot refused to build, tick or check an authority.
    ///
    /// Boxed: a `BotError` is a sum of every layer's error, and held inline it
    /// makes every `Result` in the rig larger than clippy's `result_large_err`
    /// ceiling.
    Bot(Box<BotError>),
    /// A fixed run or environment identifier did not parse.
    Id(IdError),
    /// The broker refused to register the rig's environment.
    Broker(BrokerError),
    /// The fixed flow revision did not parse.
    Flow(DigestError),
    /// The tracing facade could not be installed.
    Trace(DebugInstallError),
    /// A report line could not be written into its buffer.
    Fmt(fmt::Error),
    /// The `--json=` file could not be written.
    Io {
        /// The path that was being written.
        path: PathBuf,
        /// What the operating system said.
        cause: io::Error,
    },
    /// `--scenario` named a row the table does not hold.
    UnknownScenario {
        /// The name that was asked for.
        name: String,
        /// The names the table does hold, comma separated.
        known: String,
    },
    /// The fairness gate: the two engines did different work.
    Unfair {
        /// The scenario that was refused.
        scenario: &'static str,
        /// Which axis differed: `effects` or `evaluations`.
        what: &'static str,
        /// The bot's count on that axis.
        bot: u64,
        /// The baseline's count on that axis.
        base: u64,
    },
    /// `--scenario` was the last argument.
    MissingArg,
    /// The capability-scaling probe measured nothing.
    NoScalingPoints,
}

impl fmt::Display for BenchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Bot(ref cause) => write!(f, "{cause}"),
            Self::Id(ref cause) => write!(f, "{cause}"),
            Self::Broker(ref cause) => write!(f, "{cause}"),
            Self::Flow(ref cause) => write!(f, "{cause}"),
            Self::Trace(ref cause) => write!(f, "{cause}"),
            Self::Fmt(ref cause) => write!(f, "{cause}"),
            Self::Io {
                ref path,
                ref cause,
            } => write!(f, "writing {}: {cause}", path.display()),
            Self::UnknownScenario {
                ref name,
                ref known,
            } => write!(
                f,
                "--scenario={name} names no scenario; the table holds {known}"
            ),
            Self::Unfair {
                scenario,
                what: "effects",
                bot,
                base,
            } => write!(
                f,
                "FAIRNESS CHECK FAILED for '{scenario}': bot fired {bot} effects, baseline fired {base}. \
         The two engines are not doing the same work, so no timing from this \
         scenario is reportable."
            ),
            Self::Unfair {
                scenario,
                what: "evaluations",
                bot,
                base,
            } => write!(
                f,
                "FAIRNESS CHECK FAILED for '{scenario}': bot evaluated the condition {bot} times, \
         baseline {base} times. The effect counts agree, so this is a baseline that \
         reaches the same answer with less work -- which is exactly the comparison \
         this gate exists to refuse."
            ),
            Self::Unfair {
                scenario,
                what,
                bot,
                base,
            } => write!(
                f,
                "FAIRNESS CHECK FAILED for '{scenario}': bot {what} {bot}, baseline {base}"
            ),
            Self::MissingArg => f.write_str("--scenario needs a scenario name"),
            Self::NoScalingPoints => f.write_str("the capability-scaling run produced no points"),
        }
    }
}

impl std::error::Error for BenchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Bot(ref cause) => Some(&**cause),
            Self::Id(ref cause) => Some(cause),
            Self::Broker(ref cause) => Some(cause),
            Self::Flow(ref cause) => Some(cause),
            Self::Trace(ref cause) => Some(cause),
            Self::Fmt(ref cause) => Some(cause),
            Self::Io { ref cause, .. } => Some(cause),
            Self::UnknownScenario { .. }
            | Self::Unfair { .. }
            | Self::MissingArg
            | Self::NoScalingPoints => None,
        }
    }
}

impl From<EcsError> for BenchError {
    /// The tick, the build and the verbs fail in the world layer.
    fn from(cause: EcsError) -> Self {
        Self::Bot(Box::new(BotError::from(cause)))
    }
}

impl From<KernelError> for BenchError {
    /// An authority check or an issue refused.
    fn from(cause: KernelError) -> Self {
        Self::Bot(Box::new(BotError::from(cause)))
    }
}

impl From<IdError> for BenchError {
    fn from(cause: IdError) -> Self {
        Self::Id(cause)
    }
}

impl From<BrokerError> for BenchError {
    fn from(cause: BrokerError) -> Self {
        Self::Broker(cause)
    }
}

impl From<DigestError> for BenchError {
    fn from(cause: DigestError) -> Self {
        Self::Flow(cause)
    }
}

impl From<DebugInstallError> for BenchError {
    fn from(cause: DebugInstallError) -> Self {
        Self::Trace(cause)
    }
}

impl From<fmt::Error> for BenchError {
    fn from(cause: fmt::Error) -> Self {
        Self::Fmt(cause)
    }
}
