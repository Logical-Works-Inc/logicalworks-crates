//! The JSON verdicts: programs and jobs as values for agents (#777).
//!
//! `program_json` and `job_json` own every graph and summary a verdict
//! prints, so the binary reads fields nowhere. The shapes are stable:
//! a program carries its name, dialect, paragraphs, statements, copies,
//! unknown directives, its PERFORM, CALL and data-flow graphs and its
//! summary sentences with line ranges; a job carries its name, steps with
//! datasets, symbols, procs, includes, its job graph edges and its summary
//! sentences with line ranges.

use lgwks_std::json::{Map, Value};

use super::cobol::CobolProgram;
use super::error::LegacyError;
use super::graph::{call_graph, data_flow, job_graph, perform_graph};
use super::jcl::Job;
use super::summary::{summarize_job, summarize_program};
use super::{LegacyLimits, LineRange};

/// One program as JSON: its name, dialect, paragraphs, statements, copies,
/// unknown directives, PERFORM, CALL and data-flow graphs and summary
/// sentences with line ranges.
///
/// # Errors
///
/// A [`LegacyError`] past any ceiling, or the first refusal the graphs or
/// the summary earn.
pub fn program_json(program: &CobolProgram, limits: &LegacyLimits) -> Result<Value, LegacyError> {
    let perform = perform_graph(program, limits)?;
    let calls = call_graph(program, limits)?;
    let flow = data_flow(program, limits)?;
    let summary = summarize_program(program, limits)?;
    let mut paragraphs: Vec<Value> = Vec::new();
    for paragraph in program.paragraphs.iter() {
        paragraphs.push(object([
            ("name", Value::from(paragraph.name.as_str())),
            (
                "section",
                match paragraph.section.clone() {
                    Some(section) => Value::from(section.as_str()),
                    None => Value::Null,
                },
            ),
            ("lines", line_json(paragraph.lines)),
        ]));
    }
    let mut statements: Vec<Value> = Vec::new();
    for statement in program.statements.iter() {
        statements.push(object([
            ("kind", Value::from(statement.kind.kind_name())),
            ("text", Value::from(statement.text.as_str())),
            ("lines", line_json(statement.lines)),
        ]));
    }
    let mut copies: Vec<Value> = Vec::new();
    for copy in program.copies.iter() {
        copies.push(object([
            ("name", Value::from(copy.name.as_str())),
            ("line", Value::from(copy.line)),
            ("resolved", Value::from(copy.resolved)),
        ]));
    }
    let mut directives: Vec<Value> = Vec::new();
    for directive in program.unknown_directives.iter() {
        directives.push(object([
            ("text", Value::from(directive.text.as_str())),
            ("line", Value::from(directive.line)),
        ]));
    }
    let mut performed: Vec<Value> = Vec::new();
    for edge in perform.edges.iter() {
        performed.push(object([
            ("from", Value::from(edge.from.as_str())),
            ("to", Value::from(edge.to.as_str())),
            ("via", Value::from(transfer_name(edge.via))),
            ("lines", line_json(edge.lines)),
        ]));
    }
    let mut called: Vec<Value> = Vec::new();
    for edge in calls.edges.iter() {
        called.push(object([
            ("from", Value::from(edge.from.as_str())),
            (
                "to",
                match edge.to.clone() {
                    Some(name) => Value::from(name.as_str()),
                    None => Value::Null,
                },
            ),
            ("lines", line_json(edge.lines)),
        ]));
    }
    let mut reads: Vec<Value> = Vec::new();
    for read in flow.reads.iter() {
        reads.push(object([
            ("file", Value::from(read.file.as_str())),
            (
                "into_field",
                match read.into_field.clone() {
                    Some(field) => Value::from(field.as_str()),
                    None => Value::Null,
                },
            ),
            ("lines", line_json(read.lines)),
        ]));
    }
    let mut writes: Vec<Value> = Vec::new();
    for write in flow.writes.iter() {
        writes.push(object([
            ("record", Value::from(write.record.as_str())),
            (
                "from_field",
                match write.from_field.clone() {
                    Some(field) => Value::from(field.as_str()),
                    None => Value::Null,
                },
            ),
            ("lines", line_json(write.lines)),
        ]));
    }
    let mut moves: Vec<Value> = Vec::new();
    for movement in flow.moves.iter() {
        moves.push(object([
            ("from_field", Value::from(movement.from_field.as_str())),
            ("to_field", Value::from(movement.to_field.as_str())),
            ("lines", line_json(movement.lines)),
        ]));
    }
    Ok(object([
        ("name", Value::from(program.name.as_str())),
        ("dialect", Value::from(program.dialect.name())),
        ("paragraphs", Value::Array(paragraphs)),
        ("statements", Value::Array(statements)),
        ("copies", Value::Array(copies)),
        ("unknown_directives", Value::Array(directives)),
        (
            "graphs",
            object([
                ("perform", Value::Array(performed)),
                ("call", Value::Array(called)),
                ("reads", Value::Array(reads)),
                ("writes", Value::Array(writes)),
                ("moves", Value::Array(moves)),
            ]),
        ),
        ("sentences", sentences_json(summary.as_slice())),
    ]))
}

/// One job as JSON: its name, steps with datasets, symbols, procs, includes,
/// job graph edges and summary sentences with line ranges.
///
/// # Errors
///
/// A [`LegacyError`] past any ceiling, or the first refusal the graph or the
/// summary earns.
pub fn job_json(job: &Job, limits: &LegacyLimits) -> Result<Value, LegacyError> {
    let graph = job_graph(job, limits)?;
    let summary = summarize_job(job, limits)?;
    let mut steps: Vec<Value> = Vec::new();
    for step in job.steps.iter() {
        let mut datasets: Vec<Value> = Vec::new();
        for dataset in step.core.datasets.iter() {
            datasets.push(object([
                ("name", Value::from(dataset.name.as_str())),
                (
                    "dsn",
                    match dataset.dsn.clone() {
                        Some(name) => Value::from(name.as_str()),
                        None => Value::Null,
                    },
                ),
                ("disp", Value::from(dataset.disp.as_str())),
                ("reads", Value::from(dataset.reads)),
                ("writes", Value::from(dataset.writes)),
                ("lines", line_json(dataset.lines)),
            ]));
        }
        steps.push(object([
            ("name", Value::from(step.core.name.as_str())),
            (
                "program",
                match step.core.program.clone() {
                    Some(name) => Value::from(name.as_str()),
                    None => Value::Null,
                },
            ),
            (
                "procedure",
                match step.core.procedure.clone() {
                    Some(name) => Value::from(name.as_str()),
                    None => Value::Null,
                },
            ),
            ("overrides", symbols_json(step.core.overrides.as_slice())),
            ("datasets", Value::Array(datasets)),
            (
                "unresolved",
                Value::Array(
                    step.core
                        .unresolved
                        .iter()
                        .map(|name| Value::from(name.as_str()))
                        .collect(),
                ),
            ),
            ("note", Value::from(step.note.as_str())),
            ("lines", line_json(step.lines)),
        ]));
    }
    let mut procs: Vec<Value> = Vec::new();
    for proc_def in job.procs.iter() {
        let mut body: Vec<Value> = Vec::new();
        for inner in proc_def.steps.iter() {
            body.push(object([
                ("name", Value::from(inner.core.name.as_str())),
                (
                    "program",
                    match inner.core.program.clone() {
                        Some(name) => Value::from(name.as_str()),
                        None => Value::Null,
                    },
                ),
                (
                    "procedure",
                    match inner.core.procedure.clone() {
                        Some(name) => Value::from(name.as_str()),
                        None => Value::Null,
                    },
                ),
                ("overrides", symbols_json(inner.core.overrides.as_slice())),
                (
                    "unresolved",
                    Value::Array(
                        inner
                            .core
                            .unresolved
                            .iter()
                            .map(|name| Value::from(name.as_str()))
                            .collect(),
                    ),
                ),
            ]));
        }
        procs.push(object([
            ("name", Value::from(proc_def.name.as_str())),
            ("defaults", symbols_json(proc_def.defaults.as_slice())),
            ("steps", Value::Array(body)),
        ]));
    }
    let mut edges: Vec<Value> = Vec::new();
    for edge in graph.edges.iter() {
        edges.push(object([
            ("from", Value::from(edge.from.as_str())),
            ("to", Value::from(edge.to.as_str())),
            ("lines", line_json(edge.lines)),
        ]));
    }
    Ok(object([
        ("name", Value::from(job.name.as_str())),
        ("steps", Value::Array(steps)),
        ("symbols", symbols_json(job.symbols.as_slice())),
        ("procs", Value::Array(procs)),
        (
            "includes",
            Value::Array(
                job.includes
                    .iter()
                    .map(|include| {
                        object([
                            ("member", Value::from(include.member.as_str())),
                            ("line", Value::from(include.line)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("edges", Value::Array(edges)),
        ("sentences", sentences_json(summary.as_slice())),
    ]))
}

/// The transfer name the verdict prints: `perform` or `go`.
fn transfer_name(via: super::graph::Transfer) -> &'static str {
    match via {
        super::graph::Transfer::Perform => "perform",
        super::graph::Transfer::GoTo => "go",
    }
}

/// Sentences as JSON: the text beside the lines that prove it.
fn sentences_json(sentences: &[super::summary::Sentence]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for sentence in sentences.iter() {
        let mut ranges: Vec<Value> = Vec::new();
        for range in sentence.lines().iter() {
            ranges.push(line_json(*range));
        }
        out.push(object([
            ("text", Value::from(sentence.text())),
            ("lines", Value::Array(ranges)),
        ]));
    }
    Value::Array(out)
}

/// Symbols as JSON: name-value pairs in source order.
fn symbols_json(symbols: &[super::jcl::Symbol]) -> Value {
    Value::Array(
        symbols
            .iter()
            .map(|symbol| {
                object([
                    ("name", Value::from(symbol.name.as_str())),
                    ("value", Value::from(symbol.value.as_str())),
                ])
            })
            .collect(),
    )
}

/// One line range as JSON.
fn line_json(range: LineRange) -> Value {
    object([
        ("start", Value::from(range.start())),
        ("end", Value::from(range.end())),
    ])
}

/// A JSON object from its fields, in the order given.
fn object<const FIELDS: usize>(fields: [(&str, Value); FIELDS]) -> Value {
    Value::Object(Map::from_iter(fields))
}
