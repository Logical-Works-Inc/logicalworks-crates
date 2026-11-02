//! The graphs: PERFORM, CALL, file and field flow, and job to dataset (#777).
//!
//! Every graph is built from the parsed statements, never from drum memory:
//! a PERFORM edge runs from the paragraph holding it to the performed
//! paragraph, with the THRU end named beside it; a GO TO is a transfer edge
//! in the same graph, marked as one. A CALL edge runs from the holding
//! paragraph to the literal, or to no target for a dynamic call: the unknown
//! stays unknown. Data flow reads the SELECT, FD, READ, WRITE and MOVE
//! family: a file read names its INTO field, a record written names its FROM
//! field, and a MOVE names its movement. A job graph runs job to step, step
//! to program or procedure, and step to dataset, one edge per fact.

use super::cobol::{CallTarget, CobolProgram, StatementKind};
use super::error::LegacyError;
use super::jcl::Job;
use super::{LegacyLimits, LineRange};

/// The PERFORM graph: one edge per PERFORM and GO TO, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct PerformGraph {
    /// The edges in source order.
    pub(crate) edges: Vec<PerformEdge>,
}

/// How control moves: a PERFORM or a GO TO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transfer {
    /// A PERFORM transfer.
    Perform,
    /// A GO TO transfer.
    GoTo,
}

/// One PERFORM or GO TO transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct PerformEdge {
    /// The paragraph holding the statement, or the mainline marker.
    pub(crate) from: String,
    /// The performed paragraph, the THRU end beside it, or the GO target.
    pub(crate) to: String,
    /// How control moves.
    pub(crate) via: Transfer,
    /// The lines the statement spans.
    pub(crate) lines: LineRange,
}

/// The CALL graph: one edge per CALL, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct CallGraph {
    /// The edges in source order.
    pub(crate) edges: Vec<CallEdge>,
}

/// One CALL transfer: to a literal, or to no target when dynamic.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct CallEdge {
    /// The paragraph holding the statement, or the mainline marker.
    pub(crate) from: String,
    /// The called program, or `None` for a dynamic call with no target.
    pub(crate) to: Option<String>,
    /// The lines the statement spans.
    pub(crate) lines: LineRange,
}

/// The file and field flow: what each statement reads, writes and moves.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct DataFlow {
    /// The files read: OPEN INPUT and I-O, and READ. A SELECT only declares a
    /// file, so it is never counted as a read.
    pub(crate) reads: Vec<FlowRead>,
    /// The records written: WRITE and REWRITE, with their FROM fields.
    pub(crate) writes: Vec<FlowWrite>,
    /// The field movements: READ INTO, WRITE FROM, MOVE, COMPUTE, ADD and
    /// SUBTRACT targets.
    pub(crate) moves: Vec<FieldMove>,
}

/// One file read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct FlowRead {
    /// The file read.
    pub(crate) file: String,
    /// The INTO field, when one is named.
    pub(crate) into_field: Option<String>,
    /// The lines the statement spans.
    pub(crate) lines: LineRange,
}

/// One record written.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct FlowWrite {
    /// The record written.
    pub(crate) record: String,
    /// The FROM field, when one is named.
    pub(crate) from_field: Option<String>,
    /// The lines the statement spans.
    pub(crate) lines: LineRange,
}

/// One field movement.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct FieldMove {
    /// The source field, or the statement kind that produced the value.
    pub(crate) from_field: String,
    /// The target field.
    pub(crate) to_field: String,
    /// The lines the statement spans.
    pub(crate) lines: LineRange,
}

/// The job graph: job to step, step to program, step to dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct JobGraph {
    /// The edges in source order.
    pub(crate) edges: Vec<JobEdge>,
}

/// One job edge: a step, its program, or one of its datasets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct JobEdge {
    /// The edge source: the job, the step, or the dataset for a read.
    pub(crate) from: String,
    /// The edge target: the step, the program, or the dataset for a write.
    pub(crate) to: String,
    /// The lines the card spans.
    pub(crate) lines: LineRange,
}

/// Builds the PERFORM graph of `program`: PERFORM and GO TO transfers.
///
/// # Errors
///
/// [`LegacyError::TooManyEdges`] past the edge ceiling.
pub(crate) fn perform_graph(
    program: &CobolProgram,
    limits: &LegacyLimits,
) -> Result<PerformGraph, LegacyError> {
    let mut edges: Vec<PerformEdge> = Vec::new();
    let mut holder = String::from("(mainline)");
    for statement in program.statements.iter() {
        if is_paragraph_start(program, statement.text.as_str()) {
            holder = paragraph_at(program, statement.text.as_str());
        }
        let edge = match statement.kind {
            StatementKind::Perform {
                ref target,
                ref thru,
                conditional: _,
            } => target.clone().map(|name| PerformEdge {
                from: holder.clone(),
                to: thru_name(name.as_str(), thru.clone()),
                via: Transfer::Perform,
                lines: statement.lines,
            }),
            StatementKind::GoTo { ref target } => Some(PerformEdge {
                from: holder.clone(),
                to: target.clone(),
                via: Transfer::GoTo,
                lines: statement.lines,
            }),
            _ => None,
        };
        if let Some(edge) = edge {
            limits.check_edges(edges.len().saturating_add(1))?;
            edges.push(edge);
        }
    }
    Ok(PerformGraph { edges })
}

/// The edge target of a PERFORM: the paragraph, with its THRU end beside it.
fn thru_name(target: &str, thru: Option<String>) -> String {
    match thru {
        Some(end) => format!("{target} THRU {end}"),
        None => String::from(target),
    }
}

/// Whether `text` opens a paragraph of `program`: the paragraph table owns
/// the decision, so a statement that shares a name never moves the holder.
fn is_paragraph_start(program: &CobolProgram, text: &str) -> bool {
    let upper = super::uppercased(text);
    let words: Vec<&str> = upper.split_whitespace().collect();
    if words.len() > 2 {
        return false;
    }
    let bare = match words.first() {
        Some(word) => super::stripped(word),
        None => return false,
    };
    program.paragraphs.iter().any(|held| held.name == bare)
}

/// The paragraph `text` opens: its name, which the table holds.
fn paragraph_at(program: &CobolProgram, text: &str) -> String {
    let upper = super::uppercased(text);
    let words: Vec<&str> = upper.split_whitespace().collect();
    let bare = match words.first() {
        Some(word) => super::stripped(word),
        None => return String::from("(mainline)"),
    };
    for held in program.paragraphs.iter() {
        if held.name == bare {
            return held.name.clone();
        }
    }
    String::from("(mainline)")
}

/// Builds the CALL graph of `program`: CALL literals and dynamic gaps.
///
/// # Errors
///
/// [`LegacyError::TooManyEdges`] past the edge ceiling.
pub(crate) fn call_graph(
    program: &CobolProgram,
    limits: &LegacyLimits,
) -> Result<CallGraph, LegacyError> {
    let mut edges: Vec<CallEdge> = Vec::new();
    let mut holder = String::from("(mainline)");
    for statement in program.statements.iter() {
        if is_paragraph_start(program, statement.text.as_str()) {
            holder = paragraph_at(program, statement.text.as_str());
        }
        let edge = match statement.kind {
            StatementKind::Call { ref target } => match *target {
                CallTarget::Literal(ref name) => Some(CallEdge {
                    from: holder.clone(),
                    to: Some(name.clone()),
                    lines: statement.lines,
                }),
                CallTarget::Dynamic => Some(CallEdge {
                    from: holder.clone(),
                    to: None,
                    lines: statement.lines,
                }),
            },
            StatementKind::ExecCics { ref program, .. } => program.clone().map(|name| CallEdge {
                from: holder.clone(),
                to: Some(name),
                lines: statement.lines,
            }),
            _ => None,
        };
        if let Some(edge) = edge {
            limits.check_edges(edges.len().saturating_add(1))?;
            edges.push(edge);
        }
    }
    Ok(CallGraph { edges })
}

/// Builds the data flow of `program`: file reads and writes with their
/// fields, and every field movement the MOVE family names.
///
/// # Errors
///
/// [`LegacyError::TooManyEdges`] past the edge ceiling.
pub(crate) fn data_flow(
    program: &CobolProgram,
    limits: &LegacyLimits,
) -> Result<DataFlow, LegacyError> {
    let mut flow = DataFlow {
        reads: Vec::new(),
        writes: Vec::new(),
        moves: Vec::new(),
    };
    for statement in program.statements.iter() {
        match statement.kind {
            StatementKind::Open { ref files } => {
                for entry in files.iter() {
                    if entry.read {
                        limits.check_edges(flow.reads.len().saturating_add(1))?;
                        flow.reads.push(FlowRead {
                            file: entry.name.clone(),
                            into_field: None,
                            lines: statement.lines,
                        });
                    }
                    if entry.write {
                        limits.check_edges(flow.writes.len().saturating_add(1))?;
                        flow.writes.push(FlowWrite {
                            record: entry.name.clone(),
                            from_field: None,
                            lines: statement.lines,
                        });
                    }
                }
            }
            StatementKind::Read {
                ref file,
                ref into_field,
            } => {
                limits.check_edges(flow.reads.len().saturating_add(1))?;
                flow.reads.push(FlowRead {
                    file: file.clone(),
                    into_field: into_field.clone(),
                    lines: statement.lines,
                });
                if let Some(field) = into_field.clone() {
                    limits.check_edges(flow.moves.len().saturating_add(1))?;
                    flow.moves.push(FieldMove {
                        from_field: file.clone(),
                        to_field: field,
                        lines: statement.lines,
                    });
                }
            }
            StatementKind::Write {
                ref record,
                ref from_field,
            } => {
                limits.check_edges(flow.writes.len().saturating_add(1))?;
                flow.writes.push(FlowWrite {
                    record: record.clone(),
                    from_field: from_field.clone(),
                    lines: statement.lines,
                });
                if let Some(field) = from_field.clone() {
                    limits.check_edges(flow.moves.len().saturating_add(1))?;
                    flow.moves.push(FieldMove {
                        from_field: field,
                        to_field: record.clone(),
                        lines: statement.lines,
                    });
                }
            }
            StatementKind::Rewrite { ref record } => {
                limits.check_edges(flow.writes.len().saturating_add(1))?;
                flow.writes.push(FlowWrite {
                    record: record.clone(),
                    from_field: None,
                    lines: statement.lines,
                });
            }
            StatementKind::Move { ref targets }
            | StatementKind::Compute { ref targets }
            | StatementKind::Add { ref targets }
            | StatementKind::Subtract { ref targets } => {
                for target in targets.iter() {
                    limits.check_edges(flow.moves.len().saturating_add(1))?;
                    flow.moves.push(FieldMove {
                        from_field: String::from("(statement)"),
                        to_field: target.clone(),
                        lines: statement.lines,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(flow)
}

/// Builds the job graph of `job`: job to step, step to program or procedure,
/// and step to dataset, one edge per fact.
///
/// # Errors
///
/// [`LegacyError::TooManyEdges`] past the edge ceiling.
pub(crate) fn job_graph(job: &Job, limits: &LegacyLimits) -> Result<JobGraph, LegacyError> {
    let mut edges: Vec<JobEdge> = Vec::new();
    for step in job.steps.iter() {
        limits.check_edges(edges.len().saturating_add(1))?;
        edges.push(JobEdge {
            from: job.name.clone(),
            to: step.core.name.clone(),
            lines: step.lines,
        });
        if let Some(program) = step.core.program.clone() {
            limits.check_edges(edges.len().saturating_add(1))?;
            edges.push(JobEdge {
                from: step.core.name.clone(),
                to: program,
                lines: step.lines,
            });
        } else if let Some(procedure) = step.core.procedure.clone() {
            limits.check_edges(edges.len().saturating_add(1))?;
            edges.push(JobEdge {
                from: step.core.name.clone(),
                to: procedure,
                lines: step.lines,
            });
        }
        for dataset in step.core.datasets.iter() {
            let dsn = match dataset.dsn.clone() {
                Some(name) => name,
                None => String::from("(instream)"),
            };
            if dataset.reads {
                limits.check_edges(edges.len().saturating_add(1))?;
                edges.push(JobEdge {
                    from: dsn.clone(),
                    to: step.core.name.clone(),
                    lines: dataset.lines,
                });
            }
            if dataset.writes {
                limits.check_edges(edges.len().saturating_add(1))?;
                edges.push(JobEdge {
                    from: step.core.name.clone(),
                    to: dsn,
                    lines: dataset.lines,
                });
            }
        }
    }
    Ok(JobGraph { edges })
}
