//! Why the Restate row did not produce a number.
//!
//! Every arm is a real refusal: the server would not start or would not say it is
//! ready, its durability setting is off, an answer did not read back, or the two
//! halves of the rig disagreed about one run. None is a measurement.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why the rival row failed.
#[derive(Debug)]
pub(crate) enum RivalError {
    /// A child process could not be started.
    Spawn {
        /// The program.
        what: &'static str,
        /// What the OS said.
        source: io::Error,
    },
    /// A loopback port could not be picked by binding.
    PortBind {
        /// What the OS said.
        source: io::Error,
    },
    /// A path cannot be written into the server's config file as one literal.
    ConfigPath {
        /// The path.
        path: PathBuf,
    },
    /// `--dump-config` did not exit cleanly.
    DumpFailed {
        /// How it ended.
        status: String,
    },
    /// A line of the config dump is neither a section, a pair nor a comment.
    DumpMalformed {
        /// The line.
        line: String,
    },
    /// The dump names one key twice in one section, or opens one section twice.
    DumpDuplicate {
        /// The section.
        section: String,
        /// The key, or empty for a repeated section.
        key: String,
    },
    /// A key the durability check needs is not in the dump.
    DumpMissing {
        /// The section.
        section: &'static str,
        /// The key.
        key: &'static str,
    },
    /// The dump names a provider this check does not know the settings of.
    UnknownProvider {
        /// The provider.
        provider: String,
    },
    /// The WAL or its fsync is off for the active provider: nothing is measured.
    DurabilityOff {
        /// The section.
        section: &'static str,
        /// The key.
        key: &'static str,
        /// The value found.
        value: String,
    },
    /// A setting is neither `true` nor `false`.
    NotBoolean {
        /// The key.
        key: &'static str,
        /// The value.
        value: String,
    },
    /// A process did not answer within its bounded number of tries.
    NotReady {
        /// What was waited for.
        what: &'static str,
        /// The tries made.
        tries: u32,
    },
    /// A request could not be made or its answer could not be read.
    Http {
        /// What was asked.
        what: &'static str,
        /// The client's account.
        detail: String,
    },
    /// The server answered a status the rig does not expect.
    Status {
        /// What was asked.
        what: &'static str,
        /// The status.
        status: u16,
        /// The start of the body.
        body: String,
    },
    /// An attach answer does not read back as an output or a failure.
    AttachMalformed {
        /// The status.
        status: u16,
        /// The start of the body.
        body: String,
    },
    /// A run's ending and the worker's notice about it disagree.
    Inconsistent {
        /// The workflow key.
        key: String,
        /// How.
        why: &'static str,
    },
    /// The worker's notices stopped or their bookkeeping broke.
    Notices {
        /// Why.
        reason: &'static str,
    },
    /// A run neither parked nor ended within the bound.
    WaitTimeout {
        /// The workflow key.
        key: String,
    },
    /// A file the worker wrote does not read back.
    WorkerRecord {
        /// Which file.
        file: &'static str,
        /// The line.
        line: String,
    },
}

impl fmt::Display for RivalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Spawn { what, .. } => write!(formatter, "{what} could not be started"),
            Self::PortBind { .. } => formatter.write_str("no loopback port could be picked"),
            Self::ConfigPath { ref path } => {
                write!(formatter, "{path:?} cannot be written into a config file")
            }
            Self::DumpFailed { ref status } => {
                write!(formatter, "restate-server --dump-config ended {status}")
            }
            Self::DumpMalformed { ref line } => {
                write!(formatter, "a config dump line does not read: {line:?}")
            }
            Self::DumpDuplicate {
                ref section,
                ref key,
            } => write!(formatter, "the dump repeats [{section}] {key}"),
            Self::DumpMissing { section, key } => {
                write!(formatter, "the dump has no {key} under [{section}]")
            }
            Self::UnknownProvider { ref provider } => {
                write!(formatter, "the log provider {provider:?} is not one this check knows")
            }
            Self::DurabilityOff {
                section,
                key,
                ref value,
            } => write!(
                formatter,
                "durability is off: {key} = {value} under [{section}]; refusing to measure"
            ),
            Self::NotBoolean { key, ref value } => {
                write!(formatter, "{key} is {value:?}, not true or false")
            }
            Self::NotReady { what, tries } => {
                write!(formatter, "{what} was not ready after {tries} tries")
            }
            Self::Http { what, ref detail } => write!(formatter, "{what}: {detail}"),
            Self::Status {
                what,
                status,
                ref body,
            } => write!(formatter, "{what} answered {status}: {body}"),
            Self::AttachMalformed { status, ref body } => {
                write!(formatter, "an attach answer {status} does not read: {body:?}")
            }
            Self::Inconsistent { ref key, why } => write!(formatter, "run {key}: {why}"),
            Self::Notices { reason } => write!(formatter, "the worker's notices failed: {reason}"),
            Self::WaitTimeout { ref key } => {
                write!(formatter, "run {key} neither parked nor ended in time")
            }
            Self::WorkerRecord { file, ref line } => {
                write!(formatter, "a line of the worker's {file} does not read: {line:?}")
            }
        }
    }
}

impl Error for RivalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match *self {
            Self::Spawn { ref source, .. } | Self::PortBind { ref source } => Some(source),
            // The rest name a fact the rig observed: there is no deeper cause.
            _ => None,
        }
    }
}
