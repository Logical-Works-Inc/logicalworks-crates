//! Why the legacy COBOL and JCL readers refused their input (#777).
//!
//! Every refusal names what was read and where it sat: a byte count, a line
//! number, or the offending statement text with its line. A refusal past a
//! [`LegacyLimits`](super::LegacyLimits) ceiling is the caller raising the
//! bound; any other refusal is the input's own shape. Nothing here guesses.

/// Why a legacy source read has no program or job to return.
///
/// Each variant carries the position that earned it, so a caller reports the
/// refusal where the input went wrong rather than as a whole-file verdict.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LegacyError {
    /// The source holds more bytes than the reader admits.
    #[error("legacy source of {bytes} bytes exceeds the {limit}-byte ceiling")]
    SourceTooLarge {
        /// Observed source bytes.
        bytes: usize,
        /// The admitted ceiling in bytes.
        limit: usize,
    },
    /// The source holds more lines than the reader admits.
    #[error("legacy source of {lines} lines exceeds the {limit}-line ceiling")]
    TooManyLines {
        /// Observed line count.
        lines: usize,
        /// The admitted ceiling in lines.
        limit: usize,
    },
    /// The source holds more paragraphs, statements or steps than admitted.
    #[error("legacy source of {nodes} nodes exceeds the {limit}-node ceiling")]
    TooManyNodes {
        /// Observed node count.
        nodes: usize,
        /// The admitted ceiling in nodes.
        limit: usize,
    },
    /// A graph holds more edges than admitted.
    #[error("legacy graph of {edges} edges exceeds the {limit}-edge ceiling")]
    TooManyEdges {
        /// Observed edge count.
        edges: usize,
        /// The admitted ceiling in edges.
        limit: usize,
    },
    /// COPY expansion nests deeper than admitted.
    #[error("legacy COPY nests {depth} deep past the {limit}-level ceiling")]
    CopyTooDeep {
        /// Observed nesting depth.
        depth: usize,
        /// The admitted ceiling in levels.
        limit: usize,
    },
    /// The source names more copies than admitted.
    #[error("legacy source names {copies} copies past the {limit}-copy ceiling")]
    TooManyCopies {
        /// Observed copy count.
        copies: usize,
        /// The admitted ceiling in copies.
        limit: usize,
    },
    /// A job names more datasets than admitted.
    #[error("legacy job names {datasets} datasets past the {limit}-dataset ceiling")]
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
    #[error("jcl line {line}: statement {text:?} names no admitted operation")]
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
    #[error("a summary sentence with no lines cannot be built: {text:?}")]
    EmptySentence {
        /// The sentence text that had no evidence.
        text: String,
    },
    /// A line range was built outside the one-based line numbering.
    #[error("a line range of {start}-{end} is outside the one-based numbering")]
    BadLineRange {
        /// The refused range start.
        start: usize,
        /// The refused range end.
        end: usize,
    },
}
