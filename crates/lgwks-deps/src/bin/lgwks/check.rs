//! `lgwks check [file]`: parse and admit the file, without building it.
//!
//! The check runs the same reader and the same ceilings as the plan
//! admission: `lgwks_ast::script::parse` through `read_source`, then the byte,
//! line, step, depth and fan-out bounds `rt::apps` admits plans against. What
//! it does not do is authorize operations: a serving app names routes, not
//! operations, and there is no tenant surface to authorize against without a
//! run. Parse and bounds are the file's verdict; the registry is the run's.

use std::io::Write;
use std::process::ExitCode;

use lgwks_ast::script::{Kind, Script, StepShape};

use super::common::{self, Refusal};

/// The most bytes one app file may hold, mirroring `PlanLimits::default`.
const MAX_BYTES: usize = 65536;

/// The most steps admitted, at every level together, mirroring
/// `PlanLimits::default`. The file's line count is charged against it before
/// anything is parsed, as the admission charges the body first.
const MAX_STEPS: usize = 256;

/// The deepest nesting of steps admitted, mirroring `PlanLimits::default`.
const MAX_DEPTH: usize = 32;

/// The most children of one fan-out step, mirroring `PlanLimits::default`.
const MAX_FAN_OUT: usize = 64;

/// The command's own usage.
const CHECK_USAGE: &str = "\
lgwks check [file] [--json]

Parse and admit the app file without building it: the same reader and the
same ceilings the plan admission runs, with line:col refusals. A misspelled
word earns a did-you-mean; an unbounded construct is refused with the bound
to add. [file] is the app file, its directory, or app.rs when omitted.
";

/// Whether one fan-out step's children stay within the ceiling.
fn fans_out(step: &StepShape) -> bool {
    matches!(step.kind(), Kind::Each | Kind::Graph | Kind::Together)
}

/// One sight of a text step for the bounds walk.
struct View {
    /// Whether the step fans out.
    fan: bool,
    /// The step's children.
    children: Vec<View>,
}

/// The view of text steps, with fan-out marked.
fn view_of_shapes(steps: &[&StepShape]) -> Vec<View> {
    let mut views = Vec::with_capacity(steps.len());
    for step in steps {
        let mut children = Vec::new();
        for child in step.children() {
            children.extend(view_of_shapes(&[child]));
        }
        views.push(View {
            fan: fans_out(step),
            children,
        });
    }
    views
}

/// Walk `views` without recursion: the step count, the deepest nesting and
/// the widest fan-out.
///
/// Mirrors the plan admission's own walk, so the same file earns the same
/// numbers in both places.
fn walk(views: &[View]) -> (usize, usize, usize) {
    let mut count = 0_usize;
    let mut depth = 0_usize;
    let mut fanout = 0_usize;
    let mut stack: Vec<(&View, usize)> = views.iter().map(|view| (view, 1)).collect();
    while let Some((view, level)) = stack.pop() {
        count = count.saturating_add(1);
        depth = depth.max(level);
        if view.fan {
            fanout = fanout.max(view.children.len());
        }
        for child in view.children.iter().rev() {
            stack.push((child, level.saturating_add(1)));
        }
    }
    (count, depth, fanout)
}

/// How many lines `text` holds.
fn lines_of(text: &str) -> usize {
    text.bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        .saturating_add(1)
}

/// The bounds verdict over `text` and its parsed script, mirroring the plan
/// admission's own ceilings and messages.
fn check_bounds(text: &str, script: &Script) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    if text.len() > MAX_BYTES {
        refusals.push(Refusal::bound(format!(
            "the plan body holds {} bytes, past the ceiling of {MAX_BYTES}",
            text.len()
        )));
    }
    if lines_of(text) > MAX_STEPS {
        refusals.push(Refusal::bound(format!(
            "the plan holds {} lines, past the ceiling of {MAX_STEPS} steps",
            lines_of(text)
        )));
    }
    let top: Vec<&StepShape> = script
        .flows()
        .iter()
        .flat_map(|flow| flow.shape().steps())
        .collect();
    let views = view_of_shapes(top.as_slice());
    let (count, depth, fanout) = walk(views.as_slice());
    if count > MAX_STEPS {
        refusals.push(Refusal::bound(format!(
            "the plan holds {count} steps, past the ceiling of {MAX_STEPS}"
        )));
    }
    if depth > MAX_DEPTH {
        refusals.push(Refusal::bound(format!(
            "the plan nests {depth} deep, past the ceiling of {MAX_DEPTH}"
        )));
    }
    if fanout > MAX_FAN_OUT {
        refusals.push(Refusal::bound(format!(
            "a step fans out to {fanout}, past the ceiling of {MAX_FAN_OUT}"
        )));
    }
    refusals
}

/// Check the app file `named` names, and report the verdict.
pub(crate) fn run(
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let source = match common::open_app("check", CHECK_USAGE, args, json, out, err)? {
        Ok(source) => source,
        Err(code) => return Ok(code),
    };
    let script = match common::load_script(source.text()) {
        Ok(script) => script,
        Err(refusal) => return common::report_one(refusal, json, out, err),
    };
    let refusals = check_bounds(source.text(), &script);
    let ok = refusals.is_empty();
    common::report(ok, refusals.as_slice(), json, out, err)
}
