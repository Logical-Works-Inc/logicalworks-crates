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
///
/// # Examples
///
/// ```
/// use lgwks_ast::legacy::{LegacyLimits, cobol::parse_program, json::program_json};
///
/// let limits = LegacyLimits::new(1 << 20, 10_000, 10_000, 10_000, 64, 8, 256);
/// // Fixed format: code starts in column 8, after the sequence and indicator columns.
/// let source = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. HELLO.\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           PERFORM SHOW-IT.\n           STOP RUN.\n       SHOW-IT.\n           DISPLAY \"HI\".\n";
/// let program = parse_program(source, &limits, &|_copybook| None)?;
/// let json = program_json(&program, &limits)?;
/// assert_eq!(json.get("name").and_then(|name| name.as_str()), Some("HELLO"));
/// # Ok::<(), lgwks_ast::legacy::LegacyError>(())
/// ```
pub fn program_json(program: &CobolProgram, limits: &LegacyLimits) -> Result<Value, LegacyError> {
    let perform = perform_graph(program, limits)?;
    let calls = call_graph(program, limits)?;
    let flow = data_flow(program, limits)?;
    let summary = summarize_program(program, limits)?;
    let paragraphs = array(&program.paragraphs, |paragraph| {
        object([
            ("name", Value::from(paragraph.name.as_str())),
            ("section", text_or_null(paragraph.section.as_deref())),
            ("lines", line_json(paragraph.lines)),
        ])
    });
    let statements = array(&program.statements, |statement| {
        object([
            ("kind", Value::from(statement.kind.kind_name())),
            ("text", Value::from(statement.text.as_str())),
            ("lines", line_json(statement.lines)),
        ])
    });
    let copies = array(&program.copies, |copy| {
        object([
            ("name", Value::from(copy.name.as_str())),
            ("line", Value::from(copy.line)),
            ("resolved", Value::from(copy.resolved)),
        ])
    });
    let directives = array(&program.unknown_directives, |directive| {
        object([
            ("text", Value::from(directive.text.as_str())),
            ("line", Value::from(directive.line)),
        ])
    });
    let performed = array(&perform.edges, |edge| {
        object([
            ("from", Value::from(edge.from.as_str())),
            ("to", Value::from(edge.to.as_str())),
            ("via", Value::from(transfer_name(edge.via))),
            ("lines", line_json(edge.lines)),
        ])
    });
    let called = array(&calls.edges, |edge| {
        object([
            ("from", Value::from(edge.from.as_str())),
            ("to", text_or_null(edge.to.as_deref())),
            ("lines", line_json(edge.lines)),
        ])
    });
    let reads = array(&flow.reads, |read| {
        object([
            ("file", Value::from(read.file.as_str())),
            ("into_field", text_or_null(read.into_field.as_deref())),
            ("lines", line_json(read.lines)),
        ])
    });
    let writes = array(&flow.writes, |write| {
        object([
            ("record", Value::from(write.record.as_str())),
            ("from_field", text_or_null(write.from_field.as_deref())),
            ("lines", line_json(write.lines)),
        ])
    });
    let moves = array(&flow.moves, |movement| {
        object([
            ("from_field", Value::from(movement.from_field.as_str())),
            ("to_field", Value::from(movement.to_field.as_str())),
            ("lines", line_json(movement.lines)),
        ])
    });
    Ok(object([
        ("name", Value::from(program.name.as_str())),
        ("dialect", Value::from(program.dialect.name())),
        ("paragraphs", paragraphs),
        ("statements", statements),
        ("copies", copies),
        ("unknown_directives", directives),
        (
            "graphs",
            object([
                ("perform", performed),
                ("call", called),
                ("reads", reads),
                ("writes", writes),
                ("moves", moves),
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
///
/// # Examples
///
/// ```
/// use lgwks_ast::legacy::{LegacyLimits, jcl::parse_job, json::job_json};
///
/// let limits = LegacyLimits::new(1 << 20, 10_000, 10_000, 10_000, 64, 8, 256);
/// let source = "//PAYJOB   JOB (ACCT),'PAY'\n//STEP1    EXEC PGM=PAYROLL\n//IN       DD DSN=PAY.INPUT,DISP=SHR\n";
/// let job = parse_job(source, &limits)?;
/// let json = job_json(&job, &limits)?;
/// assert_eq!(json.get("name").and_then(|name| name.as_str()), Some("PAYJOB"));
/// # Ok::<(), lgwks_ast::legacy::LegacyError>(())
/// ```
pub fn job_json(job: &Job, limits: &LegacyLimits) -> Result<Value, LegacyError> {
    let graph = job_graph(job, limits)?;
    let summary = summarize_job(job, limits)?;
    let steps = array(&job.steps, |step| {
        let datasets = array(&step.core.datasets, |dataset| {
            object([
                ("name", Value::from(dataset.name.as_str())),
                ("dsn", text_or_null(dataset.dsn.as_deref())),
                ("disp", Value::from(dataset.disp.as_str())),
                ("reads", Value::from(dataset.reads)),
                ("writes", Value::from(dataset.writes)),
                ("lines", line_json(dataset.lines)),
            ])
        });
        object([
            ("name", Value::from(step.core.name.as_str())),
            ("program", text_or_null(step.core.program.as_deref())),
            ("procedure", text_or_null(step.core.procedure.as_deref())),
            ("overrides", symbols_json(step.core.overrides.as_slice())),
            ("datasets", datasets),
            ("unresolved", names_json(step.core.unresolved.as_slice())),
            ("note", Value::from(step.note.as_str())),
            ("lines", line_json(step.lines)),
        ])
    });
    let procs = array(&job.procs, |proc_def| {
        let body = array(&proc_def.steps, |inner| {
            object([
                ("name", Value::from(inner.core.name.as_str())),
                ("program", text_or_null(inner.core.program.as_deref())),
                ("procedure", text_or_null(inner.core.procedure.as_deref())),
                ("overrides", symbols_json(inner.core.overrides.as_slice())),
                ("unresolved", names_json(inner.core.unresolved.as_slice())),
            ])
        });
        object([
            ("name", Value::from(proc_def.name.as_str())),
            ("defaults", symbols_json(proc_def.defaults.as_slice())),
            ("steps", body),
        ])
    });
    let includes = array(&job.includes, |include| {
        object([
            ("member", Value::from(include.member.as_str())),
            ("line", Value::from(include.line)),
        ])
    });
    let edges = array(&graph.edges, |edge| {
        object([
            ("from", Value::from(edge.from.as_str())),
            ("to", Value::from(edge.to.as_str())),
            ("lines", line_json(edge.lines)),
        ])
    });
    Ok(object([
        ("name", Value::from(job.name.as_str())),
        ("steps", steps),
        ("symbols", symbols_json(job.symbols.as_slice())),
        ("procs", procs),
        ("includes", includes),
        ("edges", edges),
        ("sentences", sentences_json(summary.as_slice())),
    ]))
}

/// A JSON array holding `each` of `items`, in order.
fn array<Item>(items: &[Item], each: impl Fn(&Item) -> Value) -> Value {
    Value::Array(items.iter().map(each).collect())
}

/// A string, or `null` when there is none.
fn text_or_null(text: Option<&str>) -> Value {
    match text {
        Some(text) => Value::from(text),
        None => Value::Null,
    }
}

/// Names as a JSON array of strings, in order.
fn names_json(names: &[String]) -> Value {
    array(names, |name| Value::from(name.as_str()))
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
    array(sentences, |sentence| {
        object([
            ("text", Value::from(sentence.text())),
            ("lines", array(sentence.lines(), |range| line_json(*range))),
        ])
    })
}

/// Symbols as JSON: name-value pairs in source order.
fn symbols_json(symbols: &[super::jcl::Symbol]) -> Value {
    array(symbols, |symbol| {
        object([
            ("name", Value::from(symbol.name.as_str())),
            ("value", Value::from(symbol.value.as_str())),
        ])
    })
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
    let mut map = Map::from_iter(fields);
    if map.contains_key("") {
        let _removed = map.remove("");
    }
    Value::Object(map)
}
