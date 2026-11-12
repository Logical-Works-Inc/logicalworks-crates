//! Seeded fools against rule 9 (`partitions.rs`). A reference model draws jobs
//! of sliced and unsliced rows and steps for each suite, renders them as a
//! workflow, and names every step that runs a sliced suite without
//! `--partition`. The checker must name exactly those, in order, however the
//! text is dressed: blank lines and comments at any depth (the saboteur), `on:`
//! keys indented before `jobs:`, a stray line before the first job, an unnamed
//! step right after a named one (the wrong-order fool), two steps of one sliced
//! suite, and a fragment with no `jobs:` at all (the fat-finger). The draw is
//! `lgwks_std::seeded`, so a failing seed replays exactly.
//!
//! Source: #923 run 43232168371, where the g7 T-row step passed no
//! `--partition` and the run's wall reached 312 s of 300.

use std::error::Error;
use std::fmt::{self, Write as _};

use lgwks_std::seeded::{DrawError, Seeded};

use super::partitions::partition_problems;

/// Why a simulated workflow could not be drawn or rendered.
#[derive(Debug)]
enum SimError {
    /// The seeded stream refused a draw.
    Draw(DrawError),
    /// A line could not be written into the workflow text.
    Render(fmt::Error),
    /// A draw landed outside the table it indexes.
    Shape(&'static str),
}

impl fmt::Display for SimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Draw(ref cause) => write!(formatter, "the seeded stream refused: {cause}"),
            Self::Render(ref cause) => write!(formatter, "rendering the workflow: {cause}"),
            Self::Shape(what) => write!(formatter, "the draw is not shaped: {what}"),
        }
    }
}

impl Error for SimError {}

impl From<DrawError> for SimError {
    fn from(cause: DrawError) -> Self {
        Self::Draw(cause)
    }
}

impl From<fmt::Error> for SimError {
    fn from(cause: fmt::Error) -> Self {
        Self::Render(cause)
    }
}

/// What every test here returns.
type TestResult = Result<(), SimError>;

/// How many workflows one sweep judges.
const SEEDS: u64 = 4_096;

/// The suites a row or a step may name; one is a prefix of another.
const SUITES: [&str; 3] = ["bot-it", "bot-it-darwin", "workspace"];

/// One step the model renders: how the checker labels it, the lines it
/// renders, its condition, and whether it passes `--partition`.
struct ModelStep {
    /// The step's `name:`, or its first line when it has none.
    label: String,
    /// The workflow lines the step renders as.
    lines: Vec<String>,
    /// The text after `if:`, empty when the step always runs.
    condition: String,
    /// Whether the step's run passes `--partition`.
    partition: bool,
}

/// One job: its id, its rows (suite, sliced) and its steps.
struct ModelJob {
    /// The job's key under `jobs:`.
    id: String,
    /// Each matrix row's suite, and whether the row is a slice.
    rows: Vec<(&'static str, bool)>,
    /// The job's steps, in file order.
    steps: Vec<ModelStep>,
}

/// A blank line, a comment at the margin or an indented comment, or nothing.
fn noise(stream: &mut Seeded, text: &mut String) -> Result<(), SimError> {
    match stream.below(5)? {
        0 => text.push('\n'),
        1 => text.push_str("# a note at the margin\n"),
        2 => text.push_str("      # an indented note\n"),
        _ => {}
    }
    Ok(())
}

/// A suite drawn from [`SUITES`].
fn suite(stream: &mut Seeded) -> Result<&'static str, SimError> {
    let drawn = SUITES
        .get(stream.index(SUITES.len())?)
        .ok_or(SimError::Shape("a draw past the suites"))?;
    Ok(drawn)
}

/// One step's condition: a suite's equality, its inequality, a conjunction,
/// or none.
fn condition(stream: &mut Seeded) -> Result<String, SimError> {
    let named = suite(stream)?;
    Ok(match stream.below(4)? {
        0 => format!("matrix.suite == '{named}'"),
        1 => format!("matrix.suite != '{named}'"),
        2 => format!("matrix.suite == '{named}' && matrix.partition != ''"),
        _ => String::new(),
    })
}

/// One drawn step, named or unnamed, with or without `--partition`.
fn step(stream: &mut Seeded, index: u64) -> Result<ModelStep, SimError> {
    let condition = condition(stream)?;
    let partition = stream.below(2)? == 0;
    let run = if partition {
        "cargo nextest run ${PARTITION:+--partition $PARTITION}"
    } else {
        "cargo nextest run"
    };
    let mut lines = Vec::new();
    let label = if stream.below(2)? == 0 {
        let label = format!("Run the rows {index}");
        lines.push(format!("      - name: {label}"));
        if !condition.is_empty() {
            lines.push(format!("        if: {condition}"));
        }
        lines.push(format!("        run: {run}"));
        label
    } else {
        let label = format!("- run: {run}");
        lines.push(format!("      {label}"));
        if !condition.is_empty() {
            lines.push(format!("        if: {condition}"));
        }
        label
    };
    Ok(ModelStep {
        label,
        lines,
        condition,
        partition,
    })
}

/// One drawn job of up to three rows and four steps.
fn job(stream: &mut Seeded, index: u64) -> Result<ModelJob, SimError> {
    let mut rows = Vec::new();
    for _ in 0..=stream.below(3)? {
        rows.push((suite(stream)?, stream.below(2)? == 0));
    }
    let mut steps = Vec::new();
    for at in 0..=stream.below(4)? {
        steps.push(step(stream, at)?);
    }
    Ok(ModelJob {
        id: format!("shard_{index}"),
        rows,
        steps,
    })
}

/// The workflow text of `jobs`, dressed by the stream, and whether it is a
/// fragment with no `jobs:` key.
fn render(stream: &mut Seeded, jobs: &[ModelJob]) -> Result<(String, bool), SimError> {
    let fragment = stream.below(8)? == 0;
    let mut text = String::new();
    if !fragment {
        text.push_str("name: ci\n");
        noise(stream, &mut text)?;
        text.push_str("on:\n  pull_request:\n  push:\n    branches: [main]\n");
        noise(stream, &mut text)?;
        text.push_str("jobs:\n");
        if stream.below(3)? == 0 {
            text.push_str("    stray: before any job\n");
        }
    }
    for job in jobs {
        writeln!(text, "  {}:", job.id)?;
        noise(stream, &mut text)?;
        text.push_str(
            "    runs-on: ubuntu-latest\n    strategy:\n      matrix:\n        include:\n",
        );
        for &(row_suite, sliced) in &job.rows {
            let partition = if sliced { "slice:1/2" } else { "" };
            writeln!(
                text,
                "          - {{ suite: {row_suite}, partition: \"{partition}\" }}"
            )?;
            noise(stream, &mut text)?;
        }
        text.push_str("    steps:\n");
        for step in &job.steps {
            for line in &step.lines {
                writeln!(text, "{line}")?;
            }
            noise(stream, &mut text)?;
        }
    }
    Ok((text, fragment))
}

/// The problems the model expects: rule 9's own message for every step of a
/// job that runs a suite the job slices and passes no `--partition`.
fn expected(jobs: &[ModelJob], fragment: bool) -> Vec<String> {
    let mut problems = Vec::new();
    if fragment {
        return problems;
    }
    for job in jobs {
        let mut sliced: Vec<&str> = job
            .rows
            .iter()
            .filter(|&&(_, sliced)| sliced)
            .map(|&(row_suite, _)| row_suite)
            .collect();
        sliced.sort_unstable();
        sliced.dedup();
        for row_suite in sliced {
            let wanted = format!("matrix.suite == '{row_suite}'");
            for step in &job.steps {
                if step.condition.contains(&wanted) && !step.partition {
                    problems.push(format!(
                        "ci.yml: job {} slices suite {row_suite}, but step {:?} passes no \
                         --partition, so every slice reruns the whole set (rule 9)",
                        job.id, step.label
                    ));
                }
            }
        }
    }
    problems
}

/// One seed's workflow and the problems the model expects of it.
fn drawn(seed: u64) -> Result<(String, Vec<String>), SimError> {
    let mut stream = Seeded::from_seed(seed);
    let mut jobs = Vec::new();
    for index in 0..=stream.below(3)? {
        jobs.push(job(&mut stream, index)?);
    }
    let (text, fragment) = render(&mut stream, &jobs)?;
    Ok((text, expected(&jobs, fragment)))
}

/// Every seeded workflow gets exactly the model's problems, in order, and the
/// same seed replays to the same text and verdict.
#[test]
fn sim_rule_9_names_exactly_the_steps_the_model_names() -> TestResult {
    let (mut flagged, mut clean) = (0_u32, 0_u32);
    for seed in 0..SEEDS {
        let (text, wanted) = drawn(seed)?;
        let found = partition_problems("ci.yml", &text);
        assert_eq!(found, wanted, "seed {seed}:\n{text}");
        let (again, _) = drawn(seed)?;
        assert_eq!(again, text, "seed {seed} replays to the same workflow");
        if found.is_empty() {
            clean = clean.saturating_add(1);
        } else {
            flagged = flagged.saturating_add(1);
        }
    }
    assert!(
        flagged > 100 && clean > 100,
        "both verdicts are drawn: {flagged} flagged, {clean} clean"
    );
    Ok(())
}
