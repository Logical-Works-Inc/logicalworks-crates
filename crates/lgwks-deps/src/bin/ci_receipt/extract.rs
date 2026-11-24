//! One log in, its failures out.
//!
//! Each line is cleaned once and fed to three readers: the step tracker, the
//! nextest reader and the diagnostic reader. Test failures and diagnostics are
//! failures of their own kind; a failed step is reported only when neither found
//! anything, from the last `##[error]` of the log.
//!
//! Every text a failure carries is cut to its bound here, and a repro is written
//! only from names that are safe inside the single quotes it puts them in: a log
//! is written by the code under test, and a hostile test name must not become a
//! shell command an agent runs.

use super::compile::{Compile, CompileHit};
use super::failure::{Failure, Kind, MESSAGE_CHARS, REPRO_CHARS, STEP_CHARS};
use super::line::{clean, clip};
use super::nextest::{Nextest, TestHit};
use super::steps::{Anchor, Steps};

/// The most flag words a `NEXTEST_BUILD` repro keeps.
const MAX_FLAGS: usize = 32;

/// The failures of one log, and whether a bound cut it short.
#[derive(Debug)]
pub(crate) struct Extracted {
    /// The failures, in log order.
    pub(crate) failures: Vec<Failure>,
    /// Whether the log held more than a bound lets a receipt keep.
    pub(crate) shed: bool,
}

/// The failures `text`, the log of `job`, holds.
pub(crate) fn extract(job: &str, text: &str) -> Extracted {
    let mut steps = Steps::default();
    let mut tests = Nextest::default();
    let mut compiles = Compile::default();
    for raw in text.lines() {
        let line = clean(raw);
        steps.feed(&line);
        tests.feed(&line, steps.command());
        compiles.feed(&line, steps.command());
    }
    let (test_hits, test_shed) = tests.finish();
    let (compile_hits, compile_shed) = compiles.finish();
    let anchor = steps.anchor();
    let mut failures: Vec<Failure> = Vec::new();
    for hit in test_hits {
        let step = failing_command(anchor.as_ref(), hit.command.as_deref());
        failures.push(test_failure(job, steps.flags(), &step, hit));
    }
    for hit in compile_hits {
        let step = failing_command(anchor.as_ref(), hit.command.as_deref());
        failures.push(compile_failure(job, &step, hit));
    }
    if failures.is_empty()
        && let Some(found) = anchor
    {
        failures.push(step_failure(job, &found));
    }
    Extracted {
        failures,
        shed: test_shed || compile_shed,
    }
}

/// The command of the failing step: the anchor's, else the one running where the
/// failure was read, else none.
fn failing_command(anchor: Option<&Anchor>, own: Option<&str>) -> String {
    match anchor.and_then(|found| found.command.as_deref()).or(own) {
        Some(command) => command.to_owned(),
        None => String::new(),
    }
}

/// A failed test as a failure.
fn test_failure(job: &str, flags: Option<&str>, step: &str, hit: TestHit) -> Failure {
    let repro = test_repro(flags, &hit.binary, &hit.test);
    Failure {
        job: job.to_owned(),
        kind: Kind::Test,
        test: Some(clip(&hit.test, MESSAGE_CHARS)),
        binary: Some(clip(&hit.binary, MESSAGE_CHARS)),
        file: hit.file.map(|file| clip(&file, MESSAGE_CHARS)),
        line: hit.line,
        code: None,
        seed: hit.seed,
        message: clip(&hit.message, MESSAGE_CHARS),
        repro,
        step: clip(step, STEP_CHARS),
    }
}

/// A diagnostic as a failure.
fn compile_failure(job: &str, step: &str, hit: CompileHit) -> Failure {
    Failure {
        job: job.to_owned(),
        kind: Kind::Compile,
        test: None,
        binary: None,
        file: Some(clip(&hit.file, MESSAGE_CHARS)),
        line: Some(hit.line),
        code: hit.code.map(|code| clip(&code, MESSAGE_CHARS)),
        seed: None,
        message: clip(&hit.message, MESSAGE_CHARS),
        repro: clip(step, REPRO_CHARS),
        step: clip(step, STEP_CHARS),
    }
}

/// The failed step as a failure.
fn step_failure(job: &str, anchor: &Anchor) -> Failure {
    let command = failing_command(Some(anchor), None);
    Failure {
        job: job.to_owned(),
        kind: Kind::Step,
        test: None,
        binary: None,
        file: None,
        line: None,
        code: None,
        seed: None,
        message: clip(&anchor.message, MESSAGE_CHARS),
        repro: clip(&command, REPRO_CHARS),
        step: clip(&command, STEP_CHARS),
    }
}

/// The command that runs `test` again: with the flags of the log's `NEXTEST_BUILD`
/// when it names some, else for the package the binary id starts with. Empty when
/// a name is not safe to put inside the quotes.
pub(crate) fn test_repro(flags: Option<&str>, binary: &str, test: &str) -> String {
    if !safe_test(test) {
        return String::new();
    }
    if let Some(found) = flags
        && safe_flags(found)
    {
        return format!("cargo nextest run {found} -E 'test(={test})'");
    }
    let package = match binary.split_once("::") {
        Some((front, _)) => front,
        None => binary,
    };
    if safe_package(package) {
        format!("cargo nextest run --locked -p {package} -E 'test(={test})'")
    } else {
        String::new()
    }
}

/// Whether `name` holds only characters that mean nothing to a shell inside
/// single quotes or to nextest's filter language.
fn safe_test(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MESSAGE_CHARS
        && name.chars().all(|found| {
            found.is_ascii_alphanumeric()
                || matches!(
                    found,
                    '_' | ':' | '-' | '.' | '/' | '$' | '<' | '>' | '#' | '[' | ']'
                )
        })
}

/// Whether `flags` is a short run of plain cargo words.
fn safe_flags(flags: &str) -> bool {
    let count = flags.split_whitespace().count();
    count > 0
        && count <= MAX_FLAGS
        && flags.split_whitespace().all(|word| {
            word.chars().all(|found| {
                found.is_ascii_alphanumeric()
                    || matches!(found, '_' | '-' | '.' | '/' | ':' | '=' | ',' | '@' | '+')
            })
        })
}

/// Whether `package` is a cargo package name.
fn safe_package(package: &str) -> bool {
    !package.is_empty()
        && package.chars().count() <= MESSAGE_CHARS
        && package
            .chars()
            .all(|found| found.is_ascii_alphanumeric() || matches!(found, '_' | '-'))
}
