//! Why the legacy COBOL and JCL readers refused their input (#777).
//!
//! Every refusal names what was read and where it sat: a byte count, a line
//! number, or the offending statement text with its line. A refusal past a
//! [limits](crate::legacy::LegacyLimits) ceiling is the caller raising the
//! bound; any other refusal is the input's own shape. Nothing here guesses.

/// Why a legacy source read has no program or job to return.
///
/// Each variant carries the position that earned it, so a caller reports the
/// refusal where the input went wrong rather than as a whole-file verdict.
///
/// # Examples
///
/// ```
/// use lgwks_ast::legacy::{LegacyError, LegacyLimits, jcl::parse_job};
///
/// let tight = LegacyLimits::new(8, 10_000, 10_000, 10_000, 64, 8, 256);
/// let refused = parse_job("//PAYJOB   JOB (ACCT),'PAY'\n", &tight);
/// assert_eq!(refused, Err(LegacyError::SourceTooLarge { bytes: 28, limit: 8 }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegacyError {
    /// The source holds more bytes than the reader admits.
    SourceTooLarge {
        /// Observed source bytes.
        bytes: usize,
        /// The admitted ceiling in bytes.
        limit: usize,
    },
    /// The source holds more lines than the reader admits.
    TooManyLines {
        /// Observed line count.
        lines: usize,
        /// The admitted ceiling in lines.
        limit: usize,
    },
    /// The source holds more paragraphs, statements or steps than admitted.
    TooManyNodes {
        /// Observed node count.
        nodes: usize,
        /// The admitted ceiling in nodes.
        limit: usize,
    },
    /// A graph holds more edges than admitted.
    TooManyEdges {
        /// Observed edge count.
        edges: usize,
        /// The admitted ceiling in edges.
        limit: usize,
    },
    /// COPY expansion nests deeper than admitted.
    CopyTooDeep {
        /// Observed nesting depth.
        depth: usize,
        /// The admitted ceiling in levels.
        limit: usize,
    },
    /// The source names more copies than admitted.
    TooManyCopies {
        /// Observed copy count.
        copies: usize,
        /// The admitted ceiling in copies.
        limit: usize,
    },
    /// A job names more datasets than admitted.
    TooManyDatasets {
        /// Observed dataset count.
        datasets: usize,
        /// The admitted ceiling in datasets.
        limit: usize,
    },
    /// A JCL statement names no operation the reader admits, at its line.
    ///
    /// JCL has a closed statement set, so an unknown operation is refused
    /// rather than skipped: skipping would silently drop a step the job runs.
    UnknownStatement {
        /// One-based physical line of the statement.
        line: usize,
        /// The statement text that earned the refusal.
        text: String,
    },
    /// A summary sentence was built with no line ranges to rest on.
    ///
    /// A sentence with no lines cannot be built: every summary claim must
    /// point at the source lines that prove it.
    EmptySentence {
        /// The sentence text that had no evidence.
        text: String,
    },
    /// A line range was built outside the one-based line numbering.
    BadLineRange {
        /// The refused range start.
        start: usize,
        /// The refused range end.
        end: usize,
    },
}

impl std::fmt::Display for LegacyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::SourceTooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "legacy source of {bytes} bytes exceeds the {limit}-byte ceiling"
                )
            }
            Self::TooManyLines { lines, limit } => {
                write!(
                    formatter,
                    "legacy source of {lines} lines exceeds the {limit}-line ceiling"
                )
            }
            Self::TooManyNodes { nodes, limit } => {
                write!(
                    formatter,
                    "legacy source of {nodes} nodes exceeds the {limit}-node ceiling"
                )
            }
            Self::TooManyEdges { edges, limit } => {
                write!(
                    formatter,
                    "legacy graph of {edges} edges exceeds the {limit}-edge ceiling"
                )
            }
            Self::CopyTooDeep { depth, limit } => {
                write!(
                    formatter,
                    "legacy COPY nests {depth} deep past the {limit}-level ceiling"
                )
            }
            Self::TooManyCopies { copies, limit } => {
                write!(
                    formatter,
                    "legacy source names {copies} copies past the {limit}-copy ceiling"
                )
            }
            Self::TooManyDatasets { datasets, limit } => {
                write!(
                    formatter,
                    "legacy job names {datasets} datasets past the {limit}-dataset ceiling"
                )
            }
            Self::UnknownStatement { line, ref text } => {
                write!(
                    formatter,
                    "jcl line {line}: statement {text:?} names no admitted operation"
                )
            }
            Self::EmptySentence { ref text } => {
                write!(
                    formatter,
                    "a summary sentence with no lines cannot be built: {text:?}"
                )
            }
            Self::BadLineRange { start, end } => {
                write!(
                    formatter,
                    "a line range of {start}-{end} is outside the one-based numbering"
                )
            }
        }
    }
}

impl std::error::Error for LegacyError {}
