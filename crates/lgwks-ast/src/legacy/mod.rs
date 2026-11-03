//! The CODE half of legacy comprehension: COBOL and JCL as read, not run (#777).
//!
//! The DATA half lives in `lgwks_std::legacy` (EBCDIC pages, packed and zoned
//! decimal, copybook layouts with REDEFINES and OCCURS DEPENDING ON). This
//! module reads the code around that data: COBOL source in fixed format
//! (columns 1-6 sequence, 7 indicator, 8-72 code) and in free format
//! (`>>SOURCE FORMAT FREE`), JCL jobs with continuations and symbolics, and
//! the PERFORM, CALL, file, field and job graphs over them, with a
//! plain-language summary per program and per job whose every sentence carries
//! the line ranges it rests on.
//!
//! Three honesty rules hold everywhere. A dynamic CALL is a call whose target
//! is unknown, never a guess at one. A COPY no resolver entry names is a
//! recorded gap, never an invented body. An unknown JCL statement or an
//! unknown dialect directive is a named, located diagnostic, never another
//! dialect and never a silent skip.
//!
//! No file input or output happens here: COPY expansion reads through a
//! caller-supplied resolver, so the parser stays pure and the caller owns the
//! filesystem. Every bound is checked before the allocation it prices, and
//! past a bound the readers refuse with a
//! [typed legacy error](crate::legacy::LegacyError).

/// The COBOL source reader: fixed and free format, COPY expansion, divisions,
///
/// paragraphs and the statement set the graphs read.
pub mod cobol;
/// The dialect reader: IBM Enterprise COBOL, GnuCOBOL and Micro Focus hints,
///
/// with an unknown directive a named diagnostic rather than a guess.
pub(crate) mod dialect;
/// Why a legacy source read has no program or job to return.
pub mod error;

/// The refusal at the crate root: every reader and builder answers with it.
pub use error::LegacyError;
/// The graphs: PERFORM, CALL, file and field flow, and job to step to program
///
/// to dataset.
pub(crate) mod graph;
/// The JCL reader: jobs, steps, datasets, continuations, in-stream procs and
///
/// symbolic substitution.
pub mod jcl;
/// The JSON verdicts: programs and jobs as values for agents (#777).
///
/// `program_json` and `job_json` own every graph and summary a verdict
/// prints, so the binary reads fields nowhere.
pub mod json;
/// The plain-language summaries: one sentence per fact, every sentence pinned
///
/// to the lines that prove it.
pub(crate) mod summary;

/// The bounds every legacy reader enforces before allocating.
///
/// All fields are private; construction is through [`LegacyLimits::new`] and
/// every use is through a named check at the call site, so a refusal always
/// names the bound that earned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LegacyLimits {
    /// The most source bytes a reader admits.
    max_source_bytes: usize,
    /// The most source lines a reader admits.
    max_lines: usize,
    /// The most paragraphs, statements and steps a reader admits.
    max_nodes: usize,
    /// The most graph edges a builder admits.
    max_edges: usize,
    /// The most COPY names one program admits.
    max_copy_files: usize,
    /// The deepest COPY nesting admitted.
    max_copy_depth: usize,
    /// The most datasets one job admits.
    max_datasets: usize,
}

impl LegacyLimits {
    /// The bounds in one place: bytes, lines, nodes, edges, copies, copy
    /// depth and datasets.
    #[must_use]
    pub const fn new(
        source_bytes: usize,
        lines: usize,
        nodes: usize,
        edges: usize,
        copy_files: usize,
        copy_depth: usize,
        datasets: usize,
    ) -> Self {
        Self {
            max_source_bytes: source_bytes,
            max_lines: lines,
            max_nodes: nodes,
            max_edges: edges,
            max_copy_files: copy_files,
            max_copy_depth: copy_depth,
            max_datasets: datasets,
        }
    }

    /// Refuses `seen` bytes past the source-byte ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::SourceTooLarge`] when `seen` exceeds the ceiling.
    pub(crate) fn check_bytes(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_source_bytes {
            let refusal = Err(LegacyError::SourceTooLarge {
                bytes: seen,
                limit: self.max_source_bytes,
            });
            tracing::debug!(
                seen,
                limit = self.max_source_bytes,
                "legacy: refusing an oversize source"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` lines past the line ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::TooManyLines`] when `seen` exceeds the ceiling.
    pub(crate) fn check_lines(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_lines {
            let refusal = Err(LegacyError::TooManyLines {
                lines: seen,
                limit: self.max_lines,
            });
            tracing::debug!(
                seen,
                limit = self.max_lines,
                "legacy: refusing too many lines"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` nodes past the node ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::TooManyNodes`] when `seen` exceeds the ceiling.
    pub(crate) fn check_nodes(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_nodes {
            let refusal = Err(LegacyError::TooManyNodes {
                nodes: seen,
                limit: self.max_nodes,
            });
            tracing::debug!(
                seen,
                limit = self.max_nodes,
                "legacy: refusing too many nodes"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` edges past the edge ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::TooManyEdges`] when `seen` exceeds the ceiling.
    pub(crate) fn check_edges(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_edges {
            let refusal = Err(LegacyError::TooManyEdges {
                edges: seen,
                limit: self.max_edges,
            });
            tracing::debug!(
                seen,
                limit = self.max_edges,
                "legacy: refusing too many edges"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` copies past the copy ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::TooManyCopies`] when `seen` exceeds the ceiling.
    pub(crate) fn check_copies(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_copy_files {
            let refusal = Err(LegacyError::TooManyCopies {
                copies: seen,
                limit: self.max_copy_files,
            });
            tracing::debug!(
                seen,
                limit = self.max_copy_files,
                "legacy: refusing too many copies"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` nesting levels past the copy-depth ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::CopyTooDeep`] when `seen` exceeds the ceiling.
    pub(crate) fn check_copy_depth(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_copy_depth {
            let refusal = Err(LegacyError::CopyTooDeep {
                depth: seen,
                limit: self.max_copy_depth,
            });
            tracing::debug!(
                seen,
                limit = self.max_copy_depth,
                "legacy: refusing too deep a COPY nesting"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` datasets past the dataset ceiling.
    ///
    /// # Errors
    ///
    /// [`LegacyError::TooManyDatasets`] when `seen` exceeds the ceiling.
    pub(crate) fn check_datasets(&self, seen: usize) -> Result<(), LegacyError> {
        if seen > self.max_datasets {
            let refusal = Err(LegacyError::TooManyDatasets {
                datasets: seen,
                limit: self.max_datasets,
            });
            tracing::debug!(
                seen,
                limit = self.max_datasets,
                "legacy: refusing too many datasets"
            );
            return refusal;
        }
        Ok(())
    }
}

/// One span of physical source lines, one-based and inclusive.
///
/// A range carries the evidence for exactly one summary sentence or graph
/// edge. Statements expanded from a COPY carry the COPY statement's own
/// line: the line that brought them, which the source text holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct LineRange {
    /// The first line, one-based.
    start: usize,
    /// The last line, one-based and at or past the first.
    end: usize,
}

impl LineRange {
    /// The range from `start` to `end`, both one-based and inclusive.
    ///
    /// # Errors
    ///
    /// [`LegacyError::BadLineRange`] for a zero start or an end before the start.
    pub(crate) fn new(start: usize, end: usize) -> Result<Self, LegacyError> {
        if start == 0 || end < start {
            let refusal = Err(LegacyError::BadLineRange { start, end });
            tracing::debug!(start, end, "legacy: refusing a range outside the numbering");
            return refusal;
        }
        Ok(Self { start, end })
    }

    /// The first line, one-based.
    #[must_use]
    pub(crate) const fn start(self) -> usize {
        self.start
    }

    /// The last line, one-based and at or past the first.
    #[must_use]
    pub(crate) const fn end(self) -> usize {
        self.end
    }
}

/// The uppercased form of `text`: COBOL folds names, so the graphs key on this.
pub(crate) fn uppercased(text: &str) -> String {
    text.to_uppercase()
}

/// Whether `word` spells a decimal count of repetitions.
pub(crate) fn is_count(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|cell| cell.is_ascii_digit())
}

/// Strips one trailing period or comma from `word`, the statement punctuator.
pub(crate) fn stripped(word: &str) -> &str {
    match word.strip_suffix(['.', ',']) {
        Some(rest) => rest,
        None => word,
    }
}

/// Strips one pair of single or double quotes from `word`, the CALL literal form.
pub(crate) fn unquoted(word: &str) -> &str {
    let single = word
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''));
    match single {
        Some(rest) => rest,
        None => match word
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            Some(rest) => rest,
            None => word,
        },
    }
}

/// The line-range rule, proved beside the private type it guards (#777).
#[cfg(test)]
mod tests {
    use super::{LegacyError, LineRange};

    /// Proves a range is refused outside the one-based numbering: a zero
    /// start and an end before the start, while a valid range keeps both ends.
    #[test]
    fn a_line_range_refuses_a_zero_start_and_a_reversed_end() -> Result<(), LegacyError> {
        assert!(
            matches!(LineRange::new(0, 1), Err(LegacyError::BadLineRange { .. })),
            "a zero start is outside the one-based numbering"
        );
        assert!(
            matches!(LineRange::new(5, 4), Err(LegacyError::BadLineRange { .. })),
            "an end before the start is refused"
        );
        let range = LineRange::new(3, 5)?;
        assert_eq!(
            (range.start(), range.end()),
            (3, 5),
            "the range keeps both ends"
        );
        Ok(())
    }
}
