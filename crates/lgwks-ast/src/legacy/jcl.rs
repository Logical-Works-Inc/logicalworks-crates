//! The JCL reader: jobs, steps, datasets and symbolics (#777).
//!
//! A job is its JOB card, its SET symbolics, and its steps in order: EXEC
//! PGM steps naming a program, EXEC PROC steps naming a procedure, and DD
//! cards naming datasets with DISP. DISP decides the direction: SHR reads, OLD
//! (exclusive use) reads and writes, NEW and MOD write, and a missing DISP is
//! NEW. `*` and DATA cards hold instream data the step reads, which the reader
//! skips through to its `/*` end: instream bytes are input, not structure.
//!
//! The cards live in `cards`, the parameters in `params`: this module owns
//! the job shape and its assembly.
//!
//! Continuations join at a trailing comma onto the next `//` line, with the
//! statement's span covering every line it took. In-stream procs run to their
//! PEND, and an EXEC PROC step splices the in-stream body's steps at its own
//! line. `&NAME` symbolics substitute from SET cards and PROC defaults, with
//! EXEC overrides winning; one trailing period terminates the reference, so
//! `&HLQ..DATA` reads as the value with `.DATA` after it. An unknown
//! operation, an EXEC with neither PGM nor PROC, and a statement outside the
//! admitted set are a located [`LegacyError::UnknownStatement`]: JCL has a
//! closed statement set, so an unknown card is refused rather than skipped.

/// The JCL cards: comments dropped, continuations joined, instream skipped.
mod cards;
/// The JCL parameters: splitting, substitution and DISP direction.
mod params;

use super::error::LegacyError;
use super::{LegacyLimits, LineRange, uppercased};

use cards::Card;
use params::{include_member, read_dataset, split_pair, split_params, substitute};

/// A JCL job: its name, its steps, and the symbolics and procs they read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Job {
    /// The JOB card name, folded to uppercase.
    pub(crate) name: String,
    /// The steps in source order, in-stream PROC bodies spliced in.
    pub(crate) steps: Vec<JobStep>,
    /// The SET name-value pairs in source order.
    pub(crate) symbols: Vec<Symbol>,
    /// The in-stream procedures this job defines.
    pub(crate) procs: Vec<ProcDef>,
    /// The INCLUDE members this job names, with their lines.
    pub(crate) includes: Vec<Include>,
}

/// The EXEC target a step runs: its name, program or procedure, overrides,
/// datasets and unresolved symbolics.
///
/// One shape serves both job steps and in-stream proc body steps, so the
/// two never duplicate the field list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StepCore {
    /// The step name, or the operation for a nameless control statement.
    pub(crate) name: String,
    /// The EXEC PGM program, when one names it.
    pub(crate) program: Option<String>,
    /// The EXEC PROC procedure, when one names it.
    pub(crate) procedure: Option<String>,
    /// The EXEC card's own `NAME=value` overrides, ahead of PROC defaults.
    pub(crate) overrides: Vec<Symbol>,
    /// The step's datasets in source order.
    pub(crate) datasets: Vec<Dataset>,
    /// The symbolic names no SET, override or default supplied, in order.
    pub(crate) unresolved: Vec<String>,
}

impl StepCore {
    /// One EXEC target with no datasets yet.
    fn named(
        name: String,
        program: Option<String>,
        procedure: Option<String>,
        overrides: Vec<Symbol>,
        unresolved: Vec<String>,
    ) -> Self {
        Self {
            name,
            program,
            procedure,
            overrides,
            datasets: Vec::new(),
            unresolved,
        }
    }
}

/// One job step: an EXEC, or a control statement the job runs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct JobStep {
    /// The EXEC target the step runs.
    pub(crate) core: StepCore,
    /// A control statement's own words (`IF ...`), empty for an EXEC step.
    pub(crate) note: String,
    /// The lines the step spans.
    pub(crate) lines: LineRange,
}

/// One dataset a DD card names.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Dataset {
    /// The DD name, folded to uppercase.
    pub(crate) name: String,
    /// The DSN value, or `None` for SYSOUT, `*` and DATA cards.
    pub(crate) dsn: Option<String>,
    /// The DISP status as written: NEW, OLD, SHR, MOD, or empty.
    pub(crate) disp: String,
    /// Whether the step reads the dataset.
    pub(crate) reads: bool,
    /// Whether the step writes the dataset.
    pub(crate) writes: bool,
    /// The lines the DD card spans.
    pub(crate) lines: LineRange,
}

/// One SET name-value pair.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Symbol {
    /// The symbolic name, folded to uppercase.
    pub(crate) name: String,
    /// The value, substitution text.
    pub(crate) value: String,
}

/// One in-stream procedure: its name, its defaults, and its steps in body order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct ProcDef {
    /// The PROC name, folded to uppercase.
    pub(crate) name: String,
    /// The PROC card defaults, substituted after the EXEC overrides.
    pub(crate) defaults: Vec<Symbol>,
    /// The body's steps in body order.
    pub(crate) steps: Vec<ProcDefStep>,
}

/// One step of an in-stream procedure body.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct ProcDefStep {
    /// The EXEC target the body step runs.
    pub(crate) core: StepCore,
}

/// One INCLUDE member a job names.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Include {
    /// The member name, folded to uppercase.
    pub(crate) member: String,
    /// The one-based line of the INCLUDE card.
    pub(crate) line: usize,
}

/// Reads `source` into one job: SET symbolics, steps, procs and includes.
///
/// # Errors
///
/// A [`LegacyError`] past any ceiling, or
/// [`LegacyError::UnknownStatement`] for the first card outside the admitted
/// set, at its line.
pub fn parse_job(source: &str, limits: &LegacyLimits) -> Result<Job, LegacyError> {
    limits.check_bytes(source.len())?;
    let cards = cards::read_cards(source, limits)?;
    assemble_job(cards.as_slice(), limits)
}

/// Assembles one job from its cards: symbols first for substitution, then
/// steps, procs and includes in source order.
fn assemble_job(cards: &[Card], limits: &LegacyLimits) -> Result<Job, LegacyError> {
    let mut name = String::from("UNNAMED");
    let mut found_job = false;
    let mut symbols: Vec<Symbol> = Vec::new();
    for card in cards.iter() {
        if card.operation == "SET" {
            for pair in split_params(card.params.as_str()) {
                let (key, value) = split_pair(pair);
                if key.is_empty() {
                    continue;
                }
                limits.check_nodes(symbols.len().saturating_add(1))?;
                symbols.push(Symbol {
                    name: uppercased(key),
                    value: String::from(value),
                });
            }
        }
        if card.operation == "JOB" {
            if found_job {
                let refusal = Err(LegacyError::UnknownStatement {
                    line: card.start,
                    text: String::from("a second JOB card opens a second job"),
                });
                tracing::debug!("jcl: refusing a second JOB card in one read");
                return refusal;
            }
            found_job = true;
            if let Some(card_name) = card.name.clone() {
                name = card_name;
            }
        }
    }
    let mut job = Job {
        name,
        steps: Vec::new(),
        symbols,
        procs: Vec::new(),
        includes: Vec::new(),
    };
    let mut current_proc: Option<ProcDef> = None;
    for card in cards.iter() {
        limits.check_nodes(job.steps.len().saturating_add(job.procs.len()))?;
        if card.operation == "PROC" {
            if current_proc.is_some() {
                let refusal = Err(LegacyError::UnknownStatement {
                    line: card.start,
                    text: String::from("a PROC card opens inside another proc"),
                });
                tracing::debug!("jcl: refusing a PROC nested in a PROC");
                return refusal;
            }
            current_proc = Some(read_proc(card));
            continue;
        }
        if card.operation == "PEND" {
            match current_proc.take() {
                Some(done) => job.procs.push(done),
                None => {
                    let refusal = Err(LegacyError::UnknownStatement {
                        line: card.start,
                        text: String::from("a PEND card closes no proc"),
                    });
                    tracing::debug!("jcl: refusing a PEND with no open PROC");
                    return refusal;
                }
            }
            continue;
        }
        if current_proc.is_some() {
            read_proc_card(card, &mut current_proc, job.symbols.as_slice(), limits)?;
            continue;
        }
        read_job_card(card, &mut job, limits)?;
    }
    if current_proc.is_some() {
        let refusal = Err(LegacyError::UnknownStatement {
            line: last_line(cards),
            text: String::from("a PROC card never reaches its PEND"),
        });
        tracing::debug!("jcl: refusing a PROC with no PEND");
        return refusal;
    }
    splice_procs(&mut job, limits)?;
    Ok(job)
}

/// The last physical line the cards cover, or zero for no cards.
fn last_line(cards: &[Card]) -> usize {
    let mut end = 0_usize;
    for card in cards.iter() {
        if card.end > end {
            end = card.end;
        }
    }
    end
}

/// Reads a PROC card into its definition: its name and its defaults.
fn read_proc(card: &Card) -> ProcDef {
    let name = match card.name.clone() {
        Some(name) => name,
        None => String::from("UNNAMED"),
    };
    let mut defaults: Vec<Symbol> = Vec::new();
    for pair in split_params(card.params.as_str()) {
        let (key, value) = split_pair(pair);
        if key.is_empty() || value.is_empty() {
            continue;
        }
        defaults.push(Symbol {
            name: uppercased(key),
            value: String::from(value),
        });
    }
    ProcDef {
        name,
        defaults,
        steps: Vec::new(),
    }
}

/// Reads one body card of an open in-stream proc: EXEC and DD cards only.
fn read_proc_card(
    card: &Card,
    current: &mut Option<ProcDef>,
    symbols: &[Symbol],
    limits: &LegacyLimits,
) -> Result<(), LegacyError> {
    let proc = match current.as_mut() {
        Some(proc) => proc,
        None => {
            let refusal = Err(LegacyError::UnknownStatement {
                line: card.start,
                text: String::from("a proc body card stands outside its PROC"),
            });
            tracing::debug!("jcl: refusing a proc body card with no PROC");
            return refusal;
        }
    };
    if card.operation == "EXEC" {
        let (program, procedure, unresolved, local) = exec_target(card, symbols);
        limits.check_nodes(proc.steps.len().saturating_add(1))?;
        proc.steps.push(ProcDefStep {
            core: StepCore::named(step_name(card), program, procedure, local, unresolved),
        });
        return Ok(());
    }
    if card.operation == "DD" {
        let Some(step) = proc.steps.last_mut() else {
            let refusal = Err(no_step_error(card, " in its PROC"));
            tracing::debug!("jcl: refusing a PROC DD card before any EXEC");
            return refusal;
        };
        push_dataset(&mut step.core, card, symbols, limits)?;
        return Ok(());
    }
    let refusal = Err(LegacyError::UnknownStatement {
        line: card.start,
        text: String::from(card.params.as_str()),
    });
    tracing::debug!("jcl: refusing a card the PROC body does not admit");
    refusal
}

/// Reads one job-level card: EXEC steps, DD cards, INCLUDE members and the
/// control statements the job runs.
fn read_job_card(card: &Card, job: &mut Job, limits: &LegacyLimits) -> Result<(), LegacyError> {
    if card.operation == "JOB" || card.operation == "SET" {
        return Ok(());
    }
    if card.operation == "EXEC" {
        let (program, procedure, unresolved, local) = exec_target(card, job.symbols.as_slice());
        let range = require_range(card)?;
        let core = StepCore::named(step_name(card), program, procedure, local, unresolved);
        limits.check_nodes(job.steps.len().saturating_add(1))?;
        job.steps.push(JobStep {
            core,
            note: String::new(),
            lines: range,
        });
        if step_program_missing(job) {
            let refusal = Err(LegacyError::UnknownStatement {
                line: card.start,
                text: String::from(card.params.as_str()),
            });
            tracing::debug!("jcl: refusing an EXEC with neither PGM nor PROC");
            return refusal;
        }
        return Ok(());
    }
    if card.operation == "DD" {
        let symbols: Vec<Symbol> = job.symbols.clone();
        let Some(step) = job.steps.last_mut() else {
            let refusal = Err(no_step_error(card, ""));
            tracing::debug!("jcl: refusing a DD card before any EXEC");
            return refusal;
        };
        push_dataset(&mut step.core, card, symbols.as_slice(), limits)?;
        return Ok(());
    }
    if card.operation == "INCLUDE" {
        let member = include_member(card.params.as_str());
        limits.check_nodes(job.includes.len().saturating_add(job.steps.len()))?;
        job.includes.push(Include {
            member,
            line: card.start,
        });
        return Ok(());
    }
    if is_control(card.operation.as_str()) {
        let range = require_range(card)?;
        limits.check_nodes(job.steps.len().saturating_add(1))?;
        job.steps.push(JobStep {
            core: StepCore::named(control_name(card), None, None, Vec::new(), Vec::new()),
            note: String::from(card.params.as_str()),
            lines: range,
        });
        return Ok(());
    }
    let refusal = Err(LegacyError::UnknownStatement {
        line: card.start,
        text: String::from(card.params.as_str()),
    });
    tracing::debug!("jcl: refusing a card outside the admitted set");
    refusal
}

/// The card's line range, or the refusal a card past the line numbering earns.
fn require_range(card: &Card) -> Result<LineRange, LegacyError> {
    match card_range(card) {
        Some(range) => Ok(range),
        None => {
            let refusal = Err(LegacyError::UnknownStatement {
                line: card.start,
                text: String::from(card.params.as_str()),
            });
            tracing::debug!("jcl: refusing a card past the line numbering");
            refusal
        }
    }
}

/// The refusal a DD card with no step earns, with its PROC context.
fn no_step_error(card: &Card, context: &str) -> LegacyError {
    tracing::debug!("jcl: refusing a DD with no step{context}");
    LegacyError::UnknownStatement {
        line: card.start,
        text: String::from("a DD card stands before its step"),
    }
}

/// Pushes one DD dataset onto `core`, substituting symbolics.
fn push_dataset(
    core: &mut StepCore,
    card: &Card,
    symbols: &[Symbol],
    limits: &LegacyLimits,
) -> Result<(), LegacyError> {
    let (dataset, unresolved) = read_dataset(card, symbols)?;
    core.unresolved.extend(unresolved);
    limits.check_datasets(core.datasets.len().saturating_add(1))?;
    core.datasets.push(dataset);
    Ok(())
}

/// Whether the job's last step runs neither a program nor a procedure.
fn step_program_missing(job: &Job) -> bool {
    match job.steps.last() {
        Some(step) => {
            step.core.program.is_none() && step.core.procedure.is_none() && step.note.is_empty()
        }
        None => false,
    }
}

/// The step name: the card name, or the operation for a nameless card.
pub(crate) fn step_name(card: &Card) -> String {
    match card.name.clone() {
        Some(name) => name,
        None => card.operation.clone(),
    }
}

/// The control-statement name: the card name, or the operation.
fn control_name(card: &Card) -> String {
    step_name(card)
}

/// Whether `operation` is an admitted control statement.
fn is_control(operation: &str) -> bool {
    matches!(
        operation,
        "IF" | "THEN" | "ELSE" | "ENDIF" | "COMMAND" | "OUTPUT" | "JCLLIB" | "XMIT"
    )
}

/// The card's line range, or `None` past the line numbering.
pub(crate) fn card_range(card: &Card) -> Option<LineRange> {
    LineRange::new(card.start, card.end).ok()
}

/// The EXEC target: its PGM program or PROC procedure, substituted, with the
/// symbolic names no SET or override supplied and the card's own overrides.
fn exec_target(
    card: &Card,
    symbols: &[Symbol],
) -> (Option<String>, Option<String>, Vec<String>, Vec<Symbol>) {
    let pairs = split_params(card.params.as_str());
    let mut program: Option<String> = None;
    let mut procedure: Option<String> = None;
    let mut local: Vec<Symbol> = Vec::new();
    for pair in pairs.iter() {
        let (key, value) = split_pair(pair);
        if key == "PGM" {
            program = Some(String::from(value));
        } else if key == "PROC" {
            procedure = Some(String::from(value));
        } else if !key.is_empty() && pair.contains('=') {
            local.push(Symbol {
                name: uppercased(key),
                value: String::from(value),
            });
        }
    }
    let mut table: Vec<Symbol> = Vec::new();
    for entry in local.iter().chain(symbols.iter()) {
        if !table.iter().any(|held| held.name == entry.name) {
            table.push(Symbol {
                name: entry.name.clone(),
                value: entry.value.clone(),
            });
        }
    }
    let mut unresolved: Vec<String> = Vec::new();
    if let Some(held) = program.clone() {
        let (text, missing) = substitute(held.as_str(), table.as_slice());
        program = Some(text);
        unresolved.extend(missing);
    } else if let Some(held) = procedure.clone() {
        let (text, missing) = substitute(held.as_str(), table.as_slice());
        procedure = Some(text);
        unresolved.extend(missing);
    }
    (program, procedure, unresolved, local)
}

/// Splices in-stream PROC bodies into the steps that name them.
///
/// PROC defaults substitute after the EXEC overrides: a symbolic the EXEC
/// supplies keeps the EXEC value, and one only the PROC names takes the
/// default. What neither supplies stays unresolved on the spliced step.
fn splice_procs(job: &mut Job, limits: &LegacyLimits) -> Result<(), LegacyError> {
    let mut spliced: Vec<JobStep> = Vec::new();
    for step in job.steps.iter() {
        let wanted = match step.core.procedure.clone() {
            Some(name) => name,
            None => {
                spliced.push(step.clone());
                continue;
            }
        };
        let mut body: Option<&ProcDef> = None;
        for proc in job.procs.iter() {
            if proc.name == wanted {
                body = Some(proc);
                break;
            }
        }
        let Some(found) = body else {
            spliced.push(step.clone());
            continue;
        };
        for inner in found.steps.iter() {
            limits.check_nodes(spliced.len().saturating_add(1))?;
            limits.check_datasets(inner.core.datasets.len())?;
            let table = exec_table(
                step.core.overrides.as_slice(),
                inner.core.overrides.as_slice(),
                found.defaults.as_slice(),
            );
            let (program, missing_program) =
                fill_option(inner.core.program.clone(), table.as_slice());
            let (procedure, missing_procedure) =
                fill_option(inner.core.procedure.clone(), table.as_slice());
            let mut unresolved = step.core.unresolved.clone();
            unresolved.extend(inner.core.unresolved.clone());
            unresolved.extend(missing_program);
            unresolved.extend(missing_procedure);
            let mut datasets: Vec<Dataset> = Vec::new();
            for dataset in inner.core.datasets.iter() {
                let (dsn, missing) = fill_option(dataset.dsn.clone(), table.as_slice());
                unresolved.extend(missing);
                datasets.push(Dataset {
                    name: dataset.name.clone(),
                    dsn,
                    disp: dataset.disp.clone(),
                    reads: dataset.reads,
                    writes: dataset.writes,
                    lines: step.lines,
                });
            }
            remove_resolved(&mut unresolved, table.as_slice());
            spliced.push(JobStep {
                core: StepCore {
                    name: inner.core.name.clone(),
                    program,
                    procedure,
                    overrides: Vec::new(),
                    datasets,
                    unresolved,
                },
                note: String::new(),
                lines: step.lines,
            });
        }
    }
    job.steps = spliced;
    Ok(())
}

/// The substitution table a spliced step reads: the outer EXEC overrides
/// first, then the body EXEC overrides, then the PROC defaults, each name
/// once. An override ahead wins; the table order is the precedence.
fn exec_table(first: &[Symbol], second: &[Symbol], defaults: &[Symbol]) -> Vec<Symbol> {
    let mut table: Vec<Symbol> = Vec::new();
    for pair in first.iter().chain(second.iter()).chain(defaults.iter()) {
        if !table.iter().any(|held| held.name == pair.name) {
            table.push(Symbol {
                name: pair.name.clone(),
                value: pair.value.clone(),
            });
        }
    }
    table
}

/// Substitutes `maybe` through `table`: the text and any names still missing.
fn fill_option(maybe: Option<String>, table: &[Symbol]) -> (Option<String>, Vec<String>) {
    match maybe {
        Some(text) => {
            let (fixed, missing) = substitute(text.as_str(), table);
            (Some(fixed), missing)
        }
        None => (None, Vec::new()),
    }
}

/// Drops the names `table` supplies from `unresolved`.
fn remove_resolved(unresolved: &mut Vec<String>, table: &[Symbol]) {
    let mut kept: Vec<String> = Vec::new();
    for name in unresolved.iter() {
        let mut supplied = false;
        for entry in table.iter() {
            if entry.name == *name {
                supplied = true;
                break;
            }
        }
        if !supplied && !kept.contains(name) {
            kept.push(name.clone());
        }
    }
    *unresolved = kept;
}
