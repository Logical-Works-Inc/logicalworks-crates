//! The typed refusals of `ci-receipt`: one variant per way a run ends without a
//! receipt, each answered with exit status 2.

use std::fmt;
use std::io;
use std::path::PathBuf;

use lgwks_std::json;

use super::failure::MAX_CEILING;

/// Why `ci-receipt` wrote no receipt.
#[derive(Debug)]
pub(crate) enum ReceiptError {
    /// A word of the command line that is no flag of this tool.
    UnknownArgument(String),
    /// A flag given twice, which would make one of its values a guess.
    DuplicateFlag(&'static str),
    /// A flag with no value after it, or a flag in a value's place.
    MissingValue(&'static str),
    /// A flag the tool cannot run without.
    MissingFlag(&'static str),
    /// A `--max` that is not a whole number from 1 to the ceiling.
    BadMax(String),
    /// The logs directory could not be listed.
    LogsDir {
        /// The directory.
        path: PathBuf,
        /// What the operating system said.
        source: io::Error,
    },
    /// The logs directory holds more `.log` files than a receipt reads.
    TooManyLogs {
        /// The directory.
        path: PathBuf,
        /// The most logs one run reads.
        limit: usize,
    },
    /// A log could not be read.
    ReadLog {
        /// The log.
        path: PathBuf,
        /// What the operating system said.
        source: io::Error,
    },
    /// A receipt file could not be written.
    Write {
        /// The file.
        path: PathBuf,
        /// What the operating system said.
        source: io::Error,
    },
    /// The receipt could not be rendered as JSON.
    Render(json::Error),
}

impl fmt::Display for ReceiptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnknownArgument(ref word) => write!(formatter, "unknown argument {word:?}"),
            Self::DuplicateFlag(flag) => write!(formatter, "{flag} is given twice"),
            Self::MissingValue(flag) => write!(formatter, "{flag} needs a value"),
            Self::MissingFlag(flag) => write!(formatter, "{flag} is required"),
            Self::BadMax(ref text) => write!(
                formatter,
                "--max {text:?} is not a whole number from 1 to {MAX_CEILING}"
            ),
            Self::LogsDir {
                ref path,
                ref source,
            } => write!(formatter, "{}: {source}", path.display()),
            Self::TooManyLogs { ref path, limit } => write!(
                formatter,
                "{}: more than {limit} log files, which is more than a receipt reads",
                path.display()
            ),
            Self::ReadLog {
                ref path,
                ref source,
            }
            | Self::Write {
                ref path,
                ref source,
            } => write!(formatter, "{}: {source}", path.display()),
            Self::Render(ref source) => {
                write!(formatter, "the receipt could not be rendered: {source}")
            }
        }
    }
}

impl std::error::Error for ReceiptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::LogsDir { ref source, .. }
            | Self::ReadLog { ref source, .. }
            | Self::Write { ref source, .. } => Some(source),
            Self::Render(ref source) => Some(source),
            Self::UnknownArgument(_)
            | Self::DuplicateFlag(_)
            | Self::MissingValue(_)
            | Self::MissingFlag(_)
            | Self::BadMax(_)
            | Self::TooManyLogs { .. } => None,
        }
    }
}
