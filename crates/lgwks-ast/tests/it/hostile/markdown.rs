//! The markdown guard, proved from child processes.
//!
//! # What was measured
//!
//! tree-sitter's markdown grammar keeps its open block containers in an external
//! scanner whose state is serialized into a fixed 1 024-byte buffer, and it
//! *asserts* when that state does not fit. An assertion in a C parser is
//! `abort()`: the process dies, the caller with it, and `try_parse` never returns
//! anything a caller could handle. `"- "` repeated 255 times -- 510 bytes --
//! does it.
//!
//! Seventeen container shapes were bisected from a child process, one depth at a
//! time. **Every shape that aborts does so at 255 open containers**, whether it
//! spells them one per repetition (255 of `- `), two per repetition (128 of
//! `> - `) or three (85 of `>>> `). Three shapes -- tab runs and
//! indentation-nested blockquotes and ordered lists -- never abort at all,
//! because markdown does not nest those by indentation.
//! `MAX_MARKDOWN_CONTAINERS_PER_LINE` is 64, about a quarter of 255, and
//! `lgwks_ast::markdown_containers` counts that quantity directly.
//!
//! An exhaustive sweep afterwards took all seventeen shapes through every depth
//! from 1 to 512 -- 8 704 (shape, depth) pairs -- and every child exited.
//!
//! # What is proved here
//!
//! Every shape at bound-1, bound and bound+1, every shape at its own measured
//! abort depth, and a thousand seeded mixes of container prefixes. Each runs in a
//! child process, because a process that aborts cannot assert anything
//! afterwards: the parent reads an exit status, so "the guard held" and "the
//! guard did not hold" are two different observations rather than one green line.

use super::*;

use lgwks_ast::MAX_MARKDOWN_CONTAINERS_PER_LINE;

/// What a case builder returns: the cases, or the refusal that stopped it.
type Built<T> = Result<T, Box<dyn Error>>;

/// Set in the child so a re-executed binary takes the child branch.
const CHILD_ENV: &str = "LGWKS_AST_MARKDOWN_GUARD_CHILD";

/// Which case of [`markdown_cases`] the child is asked to parse.
const CASE_ENV: &str = "LGWKS_AST_MARKDOWN_GUARD_CASE";

/// This test's own name, for the child's `--exact` filter.
///
/// Qualified by its module, because the child is this same binary and libtest
/// matches the full path: an unqualified name selects nothing, the child runs
/// zero tests and exits 0, and the parent's "the child died" assertion then
/// fails against a child that never did anything.
const GUARD_TEST_NAME: &str =
    "hostile::markdown::markdown_never_reaches_the_scanner_past_its_bound";

/// Seeds in the mixed-prefix family.
///
/// A count rather than a seed count: the loop draws each seed from its
/// index, and a case total is compared against this on both sides.
const SEEDED_CASES: usize = 1_000;

/// The base the seeded family's seeds are derived from.
const SEED_BASE: u64 = 0x0d0c_0000_0000_0277;

/// Children run at once, at most: one per core, never more than this. Each is
/// a fresh launch of this binary that parses in milliseconds, so the launch is
/// the cost, and 1,050 of them one after another held a CI shard 57.8 s (run
/// 43158681921).
const CHILD_WAVE: usize = 8;

/// How a container shape spells one level of nesting.
#[derive(Clone, Copy)]
enum Form {
    /// The fragment repeated on a single line, then an item.
    Repeated,
    /// One item per level, indented by two spaces per level.
    Indented,
    /// A code block inside `depth` quote levels.
    Coded,
}

/// This shape's source at a nesting `depth`.
fn shape_source(form: Form, fragment: &str, depth: usize) -> String {
    match form {
        Form::Repeated => format!("{}x\n", fragment.repeat(depth)),
        Form::Indented => {
            let mut source = String::new();
            for level in 0..depth {
                source.push_str(&"  ".repeat(level));
                source.push_str(fragment);
                source.push('\n');
            }
            source
        }
        Form::Coded => format!("{}{}\n", "> ".repeat(depth), fragment),
    }
}

/// A markdown container shape: its name, the fragment it nests, how it spells a
/// level, and the smallest depth that aborted before the guard existed.
///
/// `None` for the last element means the shape never aborts, because markdown
/// does not nest it by indentation and the scanner's open-container count stays
/// flat however deep the source goes.
type Shape = (&'static str, &'static str, Form, Option<usize>);

/// Every container shape, with the depth at which it was measured to abort.
const SHAPES: [Shape; 17] = [
    ("dash-line", "- ", Form::Repeated, Some(255)),
    ("star-line", "* ", Form::Repeated, Some(255)),
    ("plus-line", "+ ", Form::Repeated, Some(255)),
    ("ordered-dot-line", "1. ", Form::Repeated, Some(255)),
    ("ordered-paren-line", "1) ", Form::Repeated, Some(255)),
    ("quote-space-line", "> ", Form::Repeated, Some(255)),
    ("quote-bare-line", ">", Form::Repeated, Some(255)),
    ("quote-triple-line", ">>> ", Form::Repeated, Some(85)),
    ("tab-line", "\t", Form::Repeated, None),
    ("indent-quote", "> x", Form::Indented, None),
    ("indent-ordered", "1. x", Form::Indented, None),
    ("indent-dash", "- x", Form::Indented, Some(255)),
    ("list-in-quote", "> - ", Form::Repeated, Some(128)),
    ("quote-in-list", "- > ", Form::Repeated, Some(128)),
    ("list-in-quote-in-list", "- > - ", Form::Repeated, Some(85)),
    ("fence-in-quotes", "```\nx\n```\n", Form::Coded, Some(255)),
    ("indented-code-in-quotes", "    x\n", Form::Coded, Some(255)),
];

/// The container fragments a seeded case mixes, drawn per line.
const FRAGMENTS: [&str; 10] = [
    "- ", "* ", "+ ", "1. ", "1) ", "> ", ">>> ", ">", "  ", "\t",
];

/// A seeded mix of container prefixes: per line, an indent and a fragment, so a
/// case can be shallow but dense, deep but sparse, or both at once.
///
/// Every draw is bounded by a count a sixteen-bit `usize` holds, and each is
/// converted rather than assumed: a host that cannot hold one is reported,
/// because a mix written from a substituted indent or fragment is still a
/// case and would still be run against the guard.
fn seeded_source(seed: u64) -> Built<String> {
    let mut rng = Rng::new(seed ^ SEED_BASE);
    let lines = rng.between(1, 24);
    let mut source = String::new();
    for _ in 0..lines {
        seeded_line(&mut rng, &mut source)?;
    }
    Ok(source)
}

/// One seeded line of a mix: an indent, then a fragment repeated, then `x`.
fn seeded_line(rng: &mut Rng, source: &mut String) -> Built<()> {
    let indent = usize::try_from(rng.between(0, 24))?;
    source.push_str(&"  ".repeat(indent));
    let count = u32::try_from(FRAGMENTS.len())?;
    let at = usize::try_from(rng.below(count))?;
    let fragment = FRAGMENTS
        .get(at)
        .copied()
        .ok_or("a draw from the fragment table names none of it")?;
    let count = usize::try_from(rng.between(1, 40))?;
    source.push_str(&fragment.repeat(count));
    source.push_str("x\n");
    Ok(())
}

/// One markdown source the guard is proved against.
struct Case {
    /// Where the case came from, as an assertion message.
    label: String,
    /// The source handed to `try_parse`.
    source: String,
    /// Whether the guard is expected to be what refuses it.
    guarded: bool,
}

/// Every case: the shapes at the bound and around it, the shapes at their own
/// measured abort depth, and [`SEEDED_CASES`] seeded mixes.
fn markdown_cases() -> Built<Vec<Case>> {
    let bound = MAX_MARKDOWN_CONTAINERS_PER_LINE;
    let mut cases = Vec::new();
    for entry in &SHAPES {
        let (name, fragment, form, aborts_at) = *entry;
        for depth in [bound.saturating_sub(1), bound, bound.saturating_add(1)] {
            cases.push(case(
                shape_source(form, fragment, depth),
                format!("{name} at depth {depth}"),
            ));
        }
        if let Some(depth) = aborts_at {
            cases.push(case(
                shape_source(form, fragment, depth),
                format!("{name} at its abort depth {depth}"),
            ));
        }
    }
    for index in 0..SEEDED_CASES {
        let seed = u64::try_from(index)?.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        cases.push(case(
            seeded_source(seed)?,
            format!("seeded mix {index} (seed {seed:#x})"),
        ));
    }
    Ok(cases)
}

/// One case from a source, recording whether the guard is what refuses it.
fn case(source: String, label: String) -> Case {
    let guarded =
        lgwks_ast::markdown_containers(&source, MAX_MARKDOWN_CONTAINERS_PER_LINE).is_some();
    Case {
        label,
        source,
        guarded,
    }
}

/// Run this binary as the child that parses case `index`, its stdout piped so
/// the parent sees it close when the child exits.
fn spawn_markdown_child(index: usize) -> Built<std::process::Child> {
    Ok(std::process::Command::new(std::env::current_exe()?)
        .args([GUARD_TEST_NAME, "--exact", "--nocapture"])
        .env(CHILD_ENV, "1")
        .env(CASE_ENV, index.to_string())
        .stdout(std::process::Stdio::piped())
        .spawn()?)
}

/// The exit status of the child parsing case `index`, spawned at `started`.
///
/// The child's exit status is the whole observation, so nothing is written to a
/// file: a code means the child answered, and no code means it did not, which is
/// the fact this module exists to record. A child that will not answer is
/// killed at [`CHILD_BUDGET`] rather than waited on, because the parse is
/// single-digit milliseconds and a hang is the defect this module records.
fn await_markdown_child(
    index: usize,
    mut child: std::process::Child,
    started: std::time::Instant,
) -> Built<std::process::ExitStatus> {
    let Some(mut output) = child.stdout.take() else {
        let _killed = child.kill();
        let _reaped = child.wait();
        let refusal = Err(format!("case {index}: the child has no stdout pipe").into());
        tracing::debug!(error = ?refusal.as_ref().err(), "hostile case: returning an error to the caller");
        return refusal;
    };
    let (answered, answer) = std::sync::mpsc::sync_channel::<()>(1);
    std::thread::scope(|scope| {
        // The child's stdout closes when it exits, so the drain ending is the
        // exit event itself: no poll and no timer stand in for it.
        scope.spawn(move || {
            let _drained = std::io::copy(&mut output, &mut std::io::sink());
            let _sent = answered.send(());
        });
        // The budget is measured from the spawn rather than added to it, so no
        // deadline is built by arithmetic that could overflow.
        match answer.recv_timeout(CHILD_BUDGET.saturating_sub(started.elapsed())) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ok(child.wait()?),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // A kill then a wait releases the child, and closing its pipe
                // ends the drain, so the scope joins and nothing is left behind.
                let _killed = child.kill();
                let _reaped = child.wait();
                let refusal =
                    Err(format!("case {index} did not answer within {CHILD_BUDGET:?}").into());
                tracing::debug!(error = ?refusal.as_ref().err(), "hostile case: returning an error to the caller");
                refusal
            }
        }
    })
}

/// The exit status of every case's child, in case order, run in waves of one
/// child per core (at most [`CHILD_WAVE`]). Every child spawned is awaited, so a
/// spawn that fails mid-wave leaves no process behind.
fn markdown_statuses(count: usize) -> Built<Vec<std::process::ExitStatus>> {
    let cores = match std::thread::available_parallelism() {
        Ok(cores) => cores.get(),
        Err(_) => 1,
    };
    let indices: Vec<usize> = (0..count).collect();
    let mut statuses = Vec::with_capacity(count);
    for wave in indices.chunks(cores.clamp(1, CHILD_WAVE)) {
        let mut running = Vec::with_capacity(wave.len());
        let mut refused = None;
        for &index in wave {
            match spawn_markdown_child(index) {
                Ok(child) => running.push((index, child, std::time::Instant::now())),
                Err(error) => {
                    refused = Some(error);
                    break;
                }
            }
        }
        let answers: Vec<Built<std::process::ExitStatus>> = running
            .into_iter()
            .map(|(index, child, started)| await_markdown_child(index, child, started))
            .collect();
        if let Some(error) = refused {
            tracing::debug!(error = %error, "hostile case: a child would not spawn");
            return Err(error);
        }
        for answer in answers {
            statuses.push(answer?);
        }
    }
    Ok(statuses)
}

/// Re-run this binary as the child that parses one case.
fn run_markdown_child() -> TestResult {
    let index = std::env::var(CASE_ENV)
        .map_err(|error| {
            format!("the child branch needs CASE_ENV to know which case to parse: {error}")
        })?
        .parse::<usize>()
        .map_err(|error| format!("CASE_ENV is not an index: {error}"))?;
    let cases = markdown_cases()?;
    let case = cases
        .get(index)
        .ok_or("CASE_ENV names a case this build does not have")?;
    // The arm is printed, not returned: a child that reaches here answered,
    // and its exit status is the observation the parent reads. The text goes
    // to stdout through `Write` because `clippy::print_stdout` is forbidden
    // workspace-wide, and this file is not an exempt surface.
    let arm = arm_of(case);
    let mut stdout = std::io::stdout();
    drop(writeln!(stdout, "{} -> {arm}", case.label));
    Ok(())
}

/// The arm a case answered with: a refusal's own text, or `accepted` for a
/// source that parsed.
fn arm_of(case: &Case) -> String {
    match try_parse(&case.source, Language::Markdown) {
        Ok(_) => String::from("accepted"),
        Err(refusal) => refusal_name(&refusal),
    }
}

#[test]
fn markdown_never_reaches_the_scanner_past_its_bound() -> TestResult {
    if std::env::var_os(CHILD_ENV).is_some() {
        return run_markdown_child();
    }

    let cases = markdown_cases()?;
    assert!(
        cases.len() >= 50 + SEEDED_CASES,
        "only {} cases; the shapes alone contribute more than 50",
        cases.len()
    );

    let statuses = markdown_statuses(cases.len())?;
    assert_eq!(statuses.len(), cases.len(), "every case's child answered");
    let mut guarded = 0_usize;
    let mut passed_through = 0_usize;
    for (case, status) in cases.iter().zip(&statuses) {
        assert!(
            status.code().is_some(),
            "{}: the child died of {status:?}. Before the guard this case called abort() inside \\
             tree-sitter's external scanner; now it must return a tree or a refusal.",
            case.label
        );
        if case.guarded {
            guarded = guarded.saturating_add(1);
            let arm = arm_of(case);
            assert_ne!(
                arm, "accepted",
                "{}: the guard is expected to refuse this source, and it parsed",
                case.label
            );
            assert!(
                arm.contains("block containers"),
                "{}: refused as `{arm}` rather than by the container bound, so this is not the \\
                 refusal the guard is being proved against",
                case.label
            );
        } else {
            passed_through = passed_through.saturating_add(1);
        }
    }

    assert!(
        guarded >= 40,
        "only {guarded} of {} cases were expected to be guarded; the shapes at their measured \\
         abort depths alone contribute more than 14",
        cases.len()
    );
    assert!(
        passed_through >= 20,
        "only {passed_through} cases were expected to pass the guard; without them a guard that \\
         refused every source would pass this test"
    );
    Ok(())
}

/// The guard's cases need the markdown grammar, so they are only meaningful
/// where it is compiled. Assert the guard rather than letting the module read as
/// covering markdown in a build without it.
#[cfg(all(not(feature = "lang-md"), feature = "lang-rust"))]
#[test]
fn the_markdown_guard_cases_are_absent_from_a_build_without_that_grammar() {
    assert!(
        Language::ALL
            .iter()
            .all(|language| language.name() != "markdown"),
        "this build compiles the markdown grammar, so the guard cases must be compiled in too; \\
         a module that skips them would read as coverage it does not have"
    );
}

/// How long a child is given before the parent stops believing it will answer.
///
/// The parse is single-digit milliseconds; a child that takes longer is a hang,
/// and a hang in the subject is exactly what this module exists to catch, so the
/// bound is asserted rather than assumed.
const CHILD_BUDGET: Duration = Duration::from_secs(30);

#[test]
fn the_child_budget_is_longer_than_a_parse_and_finite() {
    // A test that waits on a child with no timeout is the same defect as a walk
    // with no cap, one process up.
    assert!(
        CHILD_BUDGET > Duration::from_millis(1),
        "the child budget must let a parse finish"
    );
    assert!(
        CHILD_BUDGET < Duration::from_secs(600),
        "the child budget must fail a hang rather than hold the run open for it"
    );
    assert!(
        (1..=64).contains(&CHILD_WAVE),
        "a wave runs at least one child and stays bounded"
    );
}
