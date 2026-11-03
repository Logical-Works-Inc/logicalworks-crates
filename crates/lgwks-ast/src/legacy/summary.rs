//! The plain-language summaries: what each program and job does (#777).
//!
//! One sentence per fact: what the program reads, writes, calls and changes,
//! and what the job runs. Every sentence carries the line ranges it rests
//! on, and a sentence with no lines cannot be built: [`Sentence::new`]
//! refuses the empty evidence, so an ungrounded claim is unrepresentable.
//!
//! Where a fact is ambiguous the summary says it cannot tell and lists the
//! lines: a dynamic call with no target, an unresolved COPY, a REDEFINES
//! overlay read as an Other line, an unknown dialect directive, and a
//! symbolic no SET or default supplies. Never a guess.

use super::cobol::{CobolProgram, StatementKind};
use super::error::LegacyError;
use super::graph::{
    CallGraph, DataFlow, PerformGraph, Transfer, call_graph, data_flow, perform_graph,
};
use super::jcl::Job;
use super::{LegacyLimits, LineRange};

/// One summary sentence: its text and the lines that prove it.
///
/// The lines are private and validated at construction, so a sentence always
/// rests on at least one line of the source it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Sentence {
    /// The sentence text for a person.
    text: String,
    /// The line ranges the sentence rests on, never empty.
    lines: Vec<LineRange>,
}

impl Sentence {
    /// One sentence of `text` resting on `lines`.
    ///
    /// # Errors
    ///
    /// [`LegacyError::EmptySentence`] when `lines` holds no range.
    pub(crate) fn new(text: String, lines: Vec<LineRange>) -> Result<Self, LegacyError> {
        if lines.is_empty() {
            let refusal = Err(LegacyError::EmptySentence { text });
            tracing::debug!("summary: refusing a sentence with no lines");
            return refusal;
        }
        Ok(Self { text, lines })
    }

    /// The sentence text for a person.
    #[must_use]
    pub(crate) fn text(&self) -> &str {
        self.text.as_str()
    }

    /// The line ranges the sentence rests on, never empty.
    #[must_use]
    pub(crate) fn lines(&self) -> &[LineRange] {
        self.lines.as_slice()
    }
}

/// Summarizes `program`: reads, writes, calls, changes and honest gaps, one
/// sentence per fact in source order.
///
/// # Errors
///
/// A [`LegacyError`] past any ceiling, or the first ungrounded sentence the
/// summary would have to build.
pub(crate) fn summarize_program(
    program: &CobolProgram,
    limits: &LegacyLimits,
) -> Result<Vec<Sentence>, LegacyError> {
    let perform = perform_graph(program, limits)?;
    let calls = call_graph(program, limits)?;
    let flow = data_flow(program, limits)?;
    let mut sentences = summarize_reads(program, &flow, limits)?;
    sentences.extend(summarize_writes(&flow, limits)?);
    sentences.extend(summarize_calls(&calls, limits)?);
    sentences.extend(summarize_performs(&perform, limits)?);
    sentences.extend(summarize_changes(&flow, limits)?);
    sentences.extend(summarize_copies(program, limits)?);
    sentences.extend(summarize_gaps(program, limits)?);
    Ok(sentences)
}

/// One sentence per file read: SELECT declarations, OPEN INPUT modes, and READ
/// statements with their INTO fields.
fn summarize_reads(
    program: &CobolProgram,
    flow: &DataFlow,
    limits: &LegacyLimits,
) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for read in flow.reads.iter() {
        if !files.contains(&read.file) {
            files.push(read.file.clone());
        }
    }
    for file in files.iter() {
        let mut ranges: Vec<LineRange> = Vec::new();
        for read in flow.reads.iter() {
            if read.file == *file {
                ranges.push(read.lines);
            }
        }
        limits.check_nodes(out.len().saturating_add(1))?;
        out.push(Sentence::new(
            format!("Program {} reads file {}.", program.name, file),
            ranges,
        )?);
    }
    Ok(out)
}

/// One sentence per record written: WRITE and REWRITE with their FROM fields.
fn summarize_writes(flow: &DataFlow, limits: &LegacyLimits) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    let mut records: Vec<String> = Vec::new();
    for write in flow.writes.iter() {
        if !records.contains(&write.record) {
            records.push(write.record.clone());
        }
    }
    for record in records.iter() {
        let mut ranges: Vec<LineRange> = Vec::new();
        for write in flow.writes.iter() {
            if write.record == *record {
                ranges.push(write.lines);
            }
        }
        limits.check_nodes(out.len().saturating_add(1))?;
        out.push(Sentence::new(format!("The run writes {record}."), ranges)?);
    }
    Ok(out)
}

/// One sentence per call: literals by name, dynamic calls as unknown targets.
fn summarize_calls(calls: &CallGraph, limits: &LegacyLimits) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    for edge in calls.edges.iter() {
        limits.check_nodes(out.len().saturating_add(1))?;
        match edge.to.clone() {
            Some(name) => out.push(Sentence::new(
                format!("The run calls {name}."),
                vec![edge.lines],
            )?),
            None => out.push(Sentence::new(
                String::from("The run cannot tell what the dynamic call targets."),
                vec![edge.lines],
            )?),
        }
    }
    Ok(out)
}

/// One sentence per PERFORM and GO TO transfer.
fn summarize_performs(
    perform: &PerformGraph,
    limits: &LegacyLimits,
) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    for edge in perform.edges.iter() {
        limits.check_nodes(out.len().saturating_add(1))?;
        if edge.via == Transfer::GoTo {
            out.push(Sentence::new(
                format!("Control passes to {}.", edge.to),
                vec![edge.lines],
            )?);
        } else {
            out.push(Sentence::new(
                format!("The run performs {}.", edge.to),
                vec![edge.lines],
            )?);
        }
    }
    Ok(out)
}

/// One sentence per changed field: the MOVE family targets merged by field.
fn summarize_changes(flow: &DataFlow, limits: &LegacyLimits) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    for movement in flow.moves.iter() {
        if movement.from_field == "(statement)" && !fields.contains(&movement.to_field) {
            fields.push(movement.to_field.clone());
        }
    }
    for field in fields.iter() {
        let mut ranges: Vec<LineRange> = Vec::new();
        for movement in flow.moves.iter() {
            if movement.to_field == *field {
                ranges.push(movement.lines);
            }
        }
        limits.check_nodes(out.len().saturating_add(1))?;
        out.push(Sentence::new(format!("The run changes {field}."), ranges)?);
    }
    Ok(out)
}

/// One sentence per COPY: resolved copies by name, unresolved ones as gaps.
fn summarize_copies(
    program: &CobolProgram,
    limits: &LegacyLimits,
) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    for copy in program.copies.iter() {
        limits.check_nodes(out.len().saturating_add(1))?;
        let range = LineRange::new(copy.line, copy.line)?;
        if copy.resolved {
            out.push(Sentence::new(
                format!("The run includes copybook {}.", copy.name),
                vec![range],
            )?);
        } else {
            out.push(Sentence::new(
                format!(
                    "The run cannot tell what COPY {} brings: no resolver entry.",
                    copy.name
                ),
                vec![range],
            )?);
        }
    }
    Ok(out)
}

/// The honest gaps: REDEFINES overlays, unknown directives, EXEC gaps, and
/// SELECT and FD declarations with their lines.
fn summarize_gaps(
    program: &CobolProgram,
    limits: &LegacyLimits,
) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    for statement in program.statements.iter() {
        let upper = super::uppercased(statement.text.as_str());
        if upper.contains("REDEFINES") {
            limits.check_nodes(out.len().saturating_add(1))?;
            out.push(Sentence::new(
                String::from("The run cannot tell the overlaid layout: REDEFINES."),
                vec![statement.lines],
            )?);
        }
        if let StatementKind::Select {
            ref file,
            ref assign,
        } = statement.kind
        {
            limits.check_nodes(out.len().saturating_add(1))?;
            out.push(Sentence::new(
                format!("The run names file {file} assigned to {assign}."),
                vec![statement.lines],
            )?);
        }
        if let StatementKind::FileDesc { ref record } = statement.kind {
            limits.check_nodes(out.len().saturating_add(1))?;
            out.push(Sentence::new(
                format!("The run declares record {record}."),
                vec![statement.lines],
            )?);
        }
    }
    for directive in program.unknown_directives.iter() {
        limits.check_nodes(out.len().saturating_add(1))?;
        let range = LineRange::new(directive.line, directive.line)?;
        out.push(Sentence::new(
            format!(
                "The run cannot tell what directive {} means.",
                directive.text
            ),
            vec![range],
        )?);
    }
    Ok(out)
}

/// Summarizes `job`: the steps it runs and what each step reads and writes,
/// one sentence per fact in source order.
///
/// # Errors
///
/// A [`LegacyError`] past any ceiling, or the first ungrounded sentence the
/// summary would have to build.
pub(crate) fn summarize_job(
    job: &Job,
    limits: &LegacyLimits,
) -> Result<Vec<Sentence>, LegacyError> {
    let mut sentences: Vec<Sentence> = Vec::new();
    for step in job.steps.iter() {
        limits.check_nodes(sentences.len().saturating_add(1))?;
        if step.core.program.is_none() && step.core.procedure.is_none() && !step.note.is_empty() {
            sentences.push(Sentence::new(
                format!("The job checks {} {}.", step.core.name, step.note),
                vec![step.lines],
            )?);
            continue;
        }
        match step.core.program.clone() {
            Some(program) => sentences.push(Sentence::new(
                format!("The job runs step {} program {}.", step.core.name, program),
                vec![step.lines],
            )?),
            None => match step.core.procedure.clone() {
                Some(procedure) => sentences.push(Sentence::new(
                    format!(
                        "The job runs step {} procedure {}.",
                        step.core.name, procedure
                    ),
                    vec![step.lines],
                )?),
                None => sentences.push(Sentence::new(
                    format!("The job cannot tell what step {} runs.", step.core.name),
                    vec![step.lines],
                )?),
            },
        }
        for name in step.core.unresolved.iter() {
            limits.check_nodes(sentences.len().saturating_add(1))?;
            sentences.push(Sentence::new(
                format!(
                    "The job cannot tell what &{name} means in step {}.",
                    step.core.name
                ),
                vec![step.lines],
            )?);
        }
    }
    for dataset in job_datasets(job, limits)? {
        limits.check_nodes(sentences.len().saturating_add(1))?;
        sentences.push(dataset);
    }
    Ok(sentences)
}

/// One sentence per dataset the job's steps read or write.
fn job_datasets(job: &Job, limits: &LegacyLimits) -> Result<Vec<Sentence>, LegacyError> {
    let mut out: Vec<Sentence> = Vec::new();
    for step in job.steps.iter() {
        for dataset in step.core.datasets.iter() {
            limits.check_nodes(out.len().saturating_add(1))?;
            let dsn = match dataset.dsn.clone() {
                Some(name) => name,
                None => String::from("(instream)"),
            };
            if dataset.reads {
                out.push(Sentence::new(
                    format!("Step {} reads dataset {}.", step.core.name, dsn),
                    vec![dataset.lines],
                )?);
            }
            if dataset.writes {
                limits.check_nodes(out.len().saturating_add(1))?;
                out.push(Sentence::new(
                    format!("Step {} writes dataset {}.", step.core.name, dsn),
                    vec![dataset.lines],
                )?);
            }
        }
    }
    Ok(out)
}
