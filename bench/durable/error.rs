//! The failures this binary names.
//!
//! Every arm is a real failure and keeps its cause, so a caller matches the
//! variant and a reader of stderr gets one typed line, never a panic message.
//! The first seven arms are the usage class (exit 2); the rest are run failures
//! (exit 1).

use std::error::Error;
use std::fmt;
use std::io;
use std::num::ParseIntError;
use std::path::PathBuf;

use lgwks_bot::script::{DecisionError, FlowError};
use lgwks_bot::task::{HostError, StoreError};
use lgwks_std::hash::DigestParseError;
use lgwks_std::seeded::DrawError;

use crate::rival_error::RivalError;

/// The exit status of a usage failure.
pub(crate) const EXIT_USAGE: u8 = 2;

/// The exit status of a run failure.
pub(crate) const EXIT_FAILED: u8 = 1;

/// The exit status when the failure line itself could not be written to stderr.
pub(crate) const EXIT_NO_STDERR: u8 = 3;

/// How many links of a cause chain one stderr line carries.
const CAUSE_DEPTH: usize = 8;

/// Why a child process did not do what a recovery round needed of it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChildCause {
    /// The child ended before it printed its ready marker.
    ExitedBeforeReady {
        /// How the child ended, as its status reads.
        status: String,
    },
    /// The first line the child printed was not the ready marker.
    BadMarker {
        /// What the line said.
        line: String,
    },
    /// The child ended, but not by the kill signal the parent sent.
    NotKilled {
        /// How the child ended, as its status reads.
        status: String,
    },
    /// Every run the child started ended before it parked: none is left to recover.
    NothingParked,
    /// The child's stdin closed while it waited, so the parent was gone and no
    /// kill was coming.
    NeverKilled,
}

impl fmt::Display for ChildCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ExitedBeforeReady { ref status } => {
                write!(formatter, "it ended before it was ready ({status})")
            }
            Self::BadMarker { ref line } => {
                write!(
                    formatter,
                    "its first line was not the ready marker: {line:?}"
                )
            }
            Self::NotKilled { ref status } => {
                write!(formatter, "it was not ended by the kill signal ({status})")
            }
            Self::NothingParked => formatter.write_str("no run parked, so none is mid-flight"),
            Self::NeverKilled => {
                formatter.write_str("its stdin closed before it was killed, so its parent is gone")
            }
        }
    }
}

/// Why a doc named by `--write-doc` cannot take the table: nothing is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocFault {
    /// No line is the start marker outside a code fence.
    StartMissing,
    /// No line is the end marker outside a code fence.
    EndMissing,
    /// More than one line is the start marker.
    StartRepeated,
    /// More than one line is the end marker.
    EndRepeated,
    /// The end marker comes before the start marker.
    OutOfOrder,
    /// The doc is longer than [`crate::doc::MAX_DOC_BYTES`].
    TooLarge,
    /// The doc is not valid UTF-8.
    NotUtf8,
}

impl fmt::Display for DocFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::StartMissing => write!(formatter, "no line is {}", crate::doc::START),
            Self::EndMissing => write!(formatter, "no line is {}", crate::doc::END),
            Self::StartRepeated => write!(formatter, "{} is on more than one line", crate::doc::START),
            Self::EndRepeated => write!(formatter, "{} is on more than one line", crate::doc::END),
            Self::OutOfOrder => formatter.write_str("the end marker comes before the start marker"),
            Self::TooLarge => write!(formatter, "it is longer than {} bytes", crate::doc::MAX_DOC_BYTES),
            Self::NotUtf8 => formatter.write_str("it is not valid UTF-8"),
        }
    }
}

/// Why a benchmark run failed.
#[derive(Debug)]
pub(crate) enum BenchError {
    /// An argument was not valid UTF-8.
    NotUnicode,
    /// A flag this binary does not take.
    UnknownArgument {
        /// The argument as typed.
        argument: String,
    },
    /// A flag that takes a value was the last argument.
    MissingValue {
        /// The flag.
        flag: &'static str,
    },
    /// A flag's value is not a whole number.
    MalformedArgument {
        /// The flag.
        flag: &'static str,
        /// The value as typed.
        value: String,
        /// The parser's refusal.
        source: ParseIntError,
    },
    /// A flag's value is a number outside what the binary accepts.
    OutOfRange {
        /// The flag.
        flag: &'static str,
        /// The value as typed.
        value: u64,
        /// The least accepted.
        min: u64,
        /// The most accepted.
        max: u64,
    },
    /// A directory the binary was told to work in cannot be used.
    UnreadablePath {
        /// The path as given.
        path: PathBuf,
        /// What the filesystem said.
        source: io::Error,
    },
    /// The doc `--write-doc` names cannot take the table.
    DocMarkers {
        /// The doc as given.
        path: PathBuf,
        /// Why.
        fault: DocFault,
    },
    /// A read or write the run needed failed.
    Io {
        /// What was being read or written.
        context: &'static str,
        /// What the OS said.
        source: io::Error,
    },
    /// The child of a recovery round misbehaved.
    ChildFailed {
        /// How.
        cause: ChildCause,
    },
    /// The bot refused a flow-level call.
    Flow {
        /// The call.
        stage: &'static str,
        /// The bot's refusal.
        source: FlowError,
    },
    /// The host could not be built.
    Host {
        /// The call.
        stage: &'static str,
        /// The builder's refusal.
        source: HostError,
    },
    /// The run store refused.
    Store {
        /// The call.
        stage: &'static str,
        /// The store's refusal.
        source: StoreError,
    },
    /// A parked approval could not be decided.
    Decision {
        /// The host's refusal.
        source: DecisionError,
    },
    /// A digest the host named did not parse.
    Digest {
        /// The parser's refusal.
        source: DigestParseError,
    },
    /// The seeded stream refused a draw.
    Draw {
        /// The stream's refusal.
        source: DrawError,
    },
    /// The JSON document could not be written.
    Json {
        /// The writer's refusal.
        source: lgwks_std::json::Error,
    },
    /// A run ended in a way the workload does not expect.
    RunFailed {
        /// The run.
        ordinal: u32,
        /// How it ended.
        disposition: &'static str,
        /// What the report said.
        detail: String,
    },
    /// A run finished with a value the reference model does not give.
    WrongOutput {
        /// The run.
        ordinal: u32,
        /// The reference model's value.
        want: u64,
        /// The run's value.
        got: u64,
    },
    /// The reopened store holds fewer parked runs than the child reported.
    LostRuns {
        /// How many the child said were parked.
        expected: u32,
        /// How many the reopened store lists.
        found: u32,
    },
    /// A line of a file this binary wrote itself does not read back.
    MalformedRecord {
        /// Which file.
        file: &'static str,
        /// The line.
        line: String,
    },
    /// A step body ran more than once for one run.
    StepRanTwice {
        /// How many (tenant, run, step) triples ran more than once.
        duplicates: usize,
    },
    /// A recovered run never ran one of its steps.
    StepMissing {
        /// The tenant.
        tenant: String,
        /// The run.
        ordinal: u32,
        /// The step.
        step: String,
    },
    /// A worker thread ended by unwinding.
    WorkerPanicked {
        /// What the panic carried, when it carried text.
        message: String,
    },
    /// A job number named no item of the list the jobs were made from.
    OutsideJobList {
        /// The job number.
        index: u32,
    },
    /// The process resident set could not be read.
    MemoryReading {
        /// Why.
        reason: &'static str,
    },
    /// The Restate row failed.
    Rival {
        /// Why.
        source: RivalError,
    },
}

impl BenchError {
    /// The variant's name, which leads the stderr line.
    pub(crate) const fn kind(&self) -> &'static str {
        match *self {
            Self::NotUnicode => "NotUnicode",
            Self::UnknownArgument { .. } => "UnknownArgument",
            Self::MissingValue { .. } => "MissingValue",
            Self::MalformedArgument { .. } => "MalformedArgument",
            Self::OutOfRange { .. } => "OutOfRange",
            Self::UnreadablePath { .. } => "UnreadablePath",
            Self::DocMarkers { .. } => "DocMarkers",
            Self::Io { .. } => "Io",
            Self::ChildFailed { .. } => "ChildFailed",
            Self::Flow { .. } => "Flow",
            Self::Host { .. } => "Host",
            Self::Store { .. } => "Store",
            Self::Decision { .. } => "Decision",
            Self::Digest { .. } => "Digest",
            Self::Draw { .. } => "Draw",
            Self::Json { .. } => "Json",
            Self::RunFailed { .. } => "RunFailed",
            Self::WrongOutput { .. } => "WrongOutput",
            Self::LostRuns { .. } => "LostRuns",
            Self::MalformedRecord { .. } => "MalformedRecord",
            Self::StepRanTwice { .. } => "StepRanTwice",
            Self::StepMissing { .. } => "StepMissing",
            Self::WorkerPanicked { .. } => "WorkerPanicked",
            Self::OutsideJobList { .. } => "OutsideJobList",
            Self::MemoryReading { .. } => "MemoryReading",
            Self::Rival { .. } => "Rival",
        }
    }

    /// The exit status this failure ends the process with: 2 for a usage
    /// failure, 1 for every run failure.
    pub(crate) const fn exit_code(&self) -> u8 {
        if matches!(
            *self,
            Self::NotUnicode
                | Self::UnknownArgument { .. }
                | Self::MissingValue { .. }
                | Self::MalformedArgument { .. }
                | Self::OutOfRange { .. }
                | Self::UnreadablePath { .. }
                | Self::DocMarkers { .. }
        ) {
            EXIT_USAGE
        } else {
            EXIT_FAILED
        }
    }

    /// The one line stderr carries: the name, the message and the cause chain.
    pub(crate) fn line(&self) -> String {
        let mut text = format!("lgwks-bench-durable: {}: {self}", self.kind());
        let mut next = self.source();
        for _ in 0..CAUSE_DEPTH {
            let Some(cause) = next else { break };
            text.push_str(": ");
            text.push_str(&cause.to_string());
            next = cause.source();
        }
        text.replace(['\n', '\r'], " ")
    }
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NotUnicode => formatter.write_str("an argument is not valid UTF-8"),
            Self::UnknownArgument { ref argument } => {
                write!(formatter, "no flag named {argument:?}")
            }
            Self::MissingValue { flag } => write!(formatter, "{flag} needs a value"),
            Self::MalformedArgument {
                flag, ref value, ..
            } => {
                write!(formatter, "{flag} wants a whole number, got {value:?}")
            }
            Self::OutOfRange {
                flag,
                value,
                min,
                max,
            } => write!(formatter, "{flag} {value} is outside {min}..={max}"),
            Self::UnreadablePath { ref path, .. } => {
                write!(formatter, "cannot use the work directory {path:?}")
            }
            Self::DocMarkers { ref path, fault } => {
                write!(formatter, "the doc {path:?} cannot take the table: {fault}")
            }
            Self::Io { context, .. } => write!(formatter, "{context} failed"),
            Self::ChildFailed { ref cause } => {
                write!(formatter, "the recovery child failed: {cause}")
            }
            Self::Flow { stage, .. } => write!(formatter, "the bot refused at {stage}"),
            Self::Host { stage, .. } => write!(formatter, "the host could not be built at {stage}"),
            Self::Store { stage, .. } => write!(formatter, "the run store refused at {stage}"),
            Self::Decision { .. } => formatter.write_str("a parked approval could not be decided"),
            Self::Digest { .. } => formatter.write_str("a digest did not parse"),
            Self::Draw { .. } => formatter.write_str("the seeded stream refused a draw"),
            Self::Json { .. } => formatter.write_str("the JSON document could not be written"),
            Self::RunFailed {
                ordinal,
                disposition,
                ref detail,
            } => write!(formatter, "run {ordinal} ended {disposition}: {detail}"),
            Self::WrongOutput { ordinal, want, got } => {
                write!(
                    formatter,
                    "run {ordinal} returned {got}, the model says {want}"
                )
            }
            Self::LostRuns { expected, found } => write!(
                formatter,
                "the child parked {expected} runs and the reopened store lists {found}"
            ),
            Self::MalformedRecord { file, ref line } => {
                write!(formatter, "a line of {file} does not read back: {line:?}")
            }
            Self::StepRanTwice { duplicates } => {
                write!(formatter, "{duplicates} step bodies ran more than once")
            }
            Self::StepMissing {
                ref tenant,
                ordinal,
                ref step,
            } => write!(
                formatter,
                "tenant {tenant:?} run {ordinal} never ran step {step:?}"
            ),
            Self::WorkerPanicked { ref message } => {
                write!(formatter, "a worker thread panicked: {message}")
            }
            Self::OutsideJobList { index } => {
                write!(formatter, "job {index} names no item of its list")
            }
            Self::MemoryReading { reason } => {
                write!(formatter, "the resident set could not be read: {reason}")
            }
            Self::Rival { .. } => formatter.write_str("the Restate row failed"),
        }
    }
}

impl Error for BenchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match *self {
            Self::MalformedArgument { ref source, .. } => Some(source),
            Self::UnreadablePath { ref source, .. } | Self::Io { ref source, .. } => Some(source),
            Self::Flow { ref source, .. } => Some(source),
            Self::Host { ref source, .. } => Some(source),
            Self::Store { ref source, .. } => Some(source),
            Self::Decision { ref source } => Some(source),
            Self::Digest { ref source } => Some(source),
            Self::Draw { ref source } => Some(source),
            Self::Json { ref source } => Some(source),
            Self::Rival { ref source } => Some(source),
            // The rest name a fact this binary observed: there is no deeper cause.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// Every usage failure leads a one-line message and exits 2; the line never
    /// carries a newline even when the user's text does.
    #[test]
    fn usage_errors_are_one_line_and_exit_two() -> TestResult {
        let hostile = "--runs\nsecond line\r";
        let error = BenchError::UnknownArgument {
            argument: hostile.to_owned(),
        };
        let line = error.line();
        if line.contains('\n') || line.contains('\r') {
            return Err(format!("the line spans lines: {line:?}"));
        }
        if error.exit_code() != EXIT_USAGE {
            return Err("an unknown flag must exit 2".to_owned());
        }
        if !line.starts_with("lgwks-bench-durable: UnknownArgument: ") {
            return Err(format!("the line does not lead with its type: {line}"));
        }
        Ok(())
    }

    /// A cause is chained on the same line.
    #[test]
    fn the_cause_chain_rides_the_same_line() -> TestResult {
        let source = "abc".parse::<u32>().err().ok_or("abc parsed")?;
        let error = BenchError::MalformedArgument {
            flag: "--runs",
            value: "abc".to_owned(),
            source,
        };
        let line = error.line();
        if !line.contains("invalid digit") {
            return Err(format!("the parser's refusal is lost: {line}"));
        }
        if error.source().is_none() {
            return Err("a malformed number must keep its cause".to_owned());
        }
        Ok(())
    }

    /// A run failure exits 1, apart from the usage class.
    #[test]
    fn run_failures_exit_one() {
        let error = BenchError::LostRuns {
            expected: 3,
            found: 1,
        };
        assert_eq!(error.exit_code(), EXIT_FAILED);
        assert_eq!(error.kind(), "LostRuns");
    }
}
