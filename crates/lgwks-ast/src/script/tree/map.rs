//! The architecture map: one rendering for the parsed tree and the compiled
//! program (#384).
//!
//! [`write_map`] prints any steps that implement [`MapStep`], so the script
//! tool's map of a parsed [`super::Script`] and `lgwks_bot`'s `ARCHITECTURE`
//! come from the same lines of code and cannot drift apart.

use std::fmt;

use super::StepShape;

/// A step as an architecture map shows it, whoever holds it: the tree
/// [`parse`](super::super::parse) answers with, or the map `lgwks_bot::script`
/// compiles from that tree into a program.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{MapStep, parse};
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let step = &script.flows()[0].shape().steps()[0];
/// assert_eq!(MapStep::detail(step), step.detail());
/// assert_eq!(MapStep::line(step).to_string(), "2");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait MapStep: Sized {
    /// The step's one-line description, as the map prints it.
    fn detail(&self) -> &str;
    /// The source line the step was declared on.
    fn line(&self) -> impl fmt::Display;
    /// The steps nested directly inside this one.
    fn children(&self) -> &[Self];
}

impl MapStep for StepShape {
    fn detail(&self) -> &str {
        self.detail.as_str()
    }

    fn line(&self) -> impl fmt::Display {
        self.line
    }

    fn children(&self) -> &[Self] {
        self.children.as_slice()
    }
}

/// Write an architecture map: per flow, `flow <signature>  @<line>`, then each
/// step on its own line, indented two spaces per level of nesting, ending with
/// `  @<line>`.
///
/// The one rendering of a map (#384): the script tool prints a parsed
/// [`Script`](super::Script) through it and `lgwks_bot::script::Architecture` prints the
/// compiled `ARCHITECTURE` through it, so the two cannot drift apart.
///
/// # Errors
///
/// The writer's own error.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{parse, write_map};
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let mut map = String::new();
/// let flows = script.flows().iter().map(|flow| (flow.shape().signature(), flow.shape().line(), flow.shape().steps()));
/// write_map(&mut map, flows)?;
/// assert_eq!(map, script.to_string());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn write_map<'map, Out, Step, Line>(
    out: &mut Out,
    flows: impl IntoIterator<Item = (&'map str, Line, &'map [Step])>,
) -> fmt::Result
where
    Out: fmt::Write + ?Sized,
    Step: MapStep + 'map,
    Line: fmt::Display,
{
    for (signature, line, steps) in flows {
        writeln!(out, "flow {signature}  @{line}")?;
        write_steps(out, steps, 1)?;
    }
    Ok(())
}

/// Write `steps` and the steps nested in them, starting at `depth`.
fn write_steps<Out, Step>(out: &mut Out, steps: &[Step], depth: usize) -> fmt::Result
where
    Out: fmt::Write + ?Sized,
    Step: MapStep,
{
    for step in steps {
        for _ in 0..depth {
            out.write_str("  ")?;
        }
        writeln!(out, "{}  @{}", step.detail(), step.line())?;
        write_steps(out, step.children(), depth.saturating_add(1))?;
    }
    Ok(())
}
