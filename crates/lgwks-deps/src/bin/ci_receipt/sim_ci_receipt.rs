//! Seeded simulations of how `ci-receipt` reads a log, each held to a model
//! built from the generator and never from the code's own answer. The sims of
//! the receipt it writes are in `sim_ci_receipt_out.rs`.
//!
//! Source: runs 45424270372 and 45430022570, the two real failed jobs whose
//! shapes the generators vary. Fools and the bug each is for: weird-bytes, a
//! prefix or colour spelling the cleaner misreads; giant-and-empty, a log past
//! a bound; neighbour, two jobs with the same test names; monkey, lines of
//! every shape; fat-finger, a seed misread; pulled-plug, a log cut mid-line.

use std::fs;

use lgwks_std::json::{self, Value};
use lgwks_std::seeded::Seeded;

use super::ci_receipt_tests::raw;
use super::extract::extract;
use super::failure::{Kind, MAX_HITS, Receipt};
use super::fixture::Fixture;
use super::line::{MAX_LINE_CHARS, clean};
use super::sim_draw::{SEEDS, SimResult, below, log_of, paint, pick, stamp, word};
use super::{logs, render};

/// Lines of every shape a log holds and some it does not.
const FRAGMENTS: &[&str] = &[
    "        FAIL [   0.041s] ( 95/371) lgwks_ast::it m::t",
    "    thread 'm::t' panicked at src/x.rs:1:2:",
    "    left: 1",
    "##[group]Run cargo test -- --x",
    "##[error]Process completed with exit code 1.",
    "##[error]",
    "error[E0425]: cannot find",
    "error: boom",
    " --> src/lib.rs:3:5",
    "   = note: `#[deny(clippy::x)]`",
    "  NEXTEST_BUILD: --locked",
    "  NEXTEST_BUILD: --x; rm",
    "     Summary [  1.0s] 1/2",
    "        PASS [   0.1s] (1/2) a b",
    "",
    "^[[31;1m",
    "\u{1b}[",
    "FAIL [",
    "TRY 2 FAIL [ 1s] (1/1) a b",
    "seed=42",
    "\u{65e5}\u{672c}\u{8a9e}",
    "\u{2500}\u{2500}\u{2500}\u{2500}",
];

/// Source: runs 45424270372 and 45430022570. Fool: monkey. Bug caught: a line of
/// no known shape panics a reader, differs between two reads, or yields a field
/// past its bound or a receipt that is not JSON.
#[test]
fn sim_ci_receipt_garbage_is_bounded_deterministic_and_renders() -> SimResult {
    let tail_alphabet = [
        'x', '\u{e9}', '\u{65e5}', '|', '\'', '`', '@', '\u{1b}', '^', '[',
    ];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let mut lines = Vec::new();
        for _ in 0..200 {
            let fragment = *pick(&mut draw, FRAGMENTS)?;
            let length = below(&mut draw, 12)?;
            let tail = word(&mut draw, &tail_alphabet, length)?;
            lines.push(format!("{fragment}{tail}"));
        }
        let log = log_of(&mut draw, &lines)?;
        let once = extract("g", &log);
        let twice = extract("g", &log);
        assert_eq!(
            once.failures, twice.failures,
            "seed {seed}: two reads differ"
        );
        for found in &once.failures {
            let bounded = found.message.chars().count() <= 400
                && found.step.chars().count() <= 200
                && found.repro.chars().count() <= 1_000;
            assert!(bounded, "seed {seed}: a field past its bound: {found:?}");
        }
        let mut receipt = Receipt::new(10);
        receipt.absorb(once.failures);
        let parsed = json::from_str::<Value>(&render::json(&receipt)?)?;
        let held = parsed
            .get("failures")
            .and_then(Value::as_array)
            .map(Vec::len);
        assert!(
            held.is_some_and(|count| count <= 10),
            "seed {seed}: the receipt is not JSON of at most 10"
        );
    }
    Ok(())
}

/// The statuses of a test that did not pass.
const FAILING: [&str; 5] = ["FAIL", "TIMEOUT", "SIGSEGV", "SIGABRT", "LEAK"];

/// The statuses of lines that are no failure.
const NOISE: [&str; 5] = ["PASS", "SKIP", "SLOW", "START", "SETUP"];

/// Source: run 45424270372. Fool: neighbour. Bug caught: a failure missed, made
/// twice by its `Summary` repeat or a retry, taken from a passing line, or given
/// to the wrong job when two jobs fail the same test names.
#[test]
fn sim_ci_receipt_planted_failures_are_exactly_the_failures_found_in_each_job() -> SimResult {
    let binaries = ["a::it", "b", "lgwks_deps::bin/x-y"];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let count = below(&mut draw, 9)?;
        let mut lines = vec!["        PASS [   0.1s] ( 1/9) a::it noise::first".to_owned()];
        let mut planted: Vec<(String, String)> = Vec::new();
        let mut panics: Vec<(String, u32)> = Vec::new();
        let mut repeats = Vec::new();
        for index in 0..count {
            let binary = *pick(&mut draw, &binaries)?;
            let test = format!("m{}::t{index}", below(&mut draw, 4)?);
            let failing = *pick(&mut draw, &FAILING)?;
            let label = paint(&mut draw, failing)?;
            let pad = *pick(&mut draw, &["        ", "    TRY 1 "])?;
            lines.push(format!(
                "{pad}{label} [   0.0{index}s] ({index:>3}/999) {binary} {test}"
            ));
            if below(&mut draw, 2)? == 0 {
                let line = u32::try_from(index)?.saturating_add(1);
                lines.push(format!(
                    "    thread 'x' panicked at src/s{index}.rs:{line}:1:"
                ));
                lines.push("    boom".to_owned());
                panics.push((test.clone(), line));
            }
            let other = *pick(&mut draw, &NOISE)?;
            lines.push(format!(
                "        {other} [   0.1s] ({index:>3}/999) {binary} noise::p{index}"
            ));
            repeats.push(format!(
                "        FAIL [   0.0{index}s] ({index:>3}/999) {binary} {test}"
            ));
            planted.push((binary.to_owned(), test));
        }
        lines.push("\u{2500}\u{2500}\u{2500}\u{2500}".to_owned());
        lines.push("     Summary [  1.0s] 9/9 tests run".to_owned());
        lines.extend(repeats);
        let log = log_of(&mut draw, &lines)?;
        planted.sort();
        let mut receipt = Receipt::new(100);
        for job in ["j1", "j2"] {
            let found = extract(job, &log);
            let mut names: Vec<(String, String)> = found
                .failures
                .iter()
                .filter_map(|each| each.binary.clone().zip(each.test.clone()))
                .collect();
            names.sort();
            assert_eq!(
                names, planted,
                "seed {seed} {job}: the failures found are the ones planted"
            );
            for entry in &panics {
                let test = entry.0.as_str();
                let hit = found
                    .failures
                    .iter()
                    .find(|each| each.test.as_deref() == Some(test));
                let read = hit.and_then(|each| each.line);
                assert_eq!(read, Some(entry.1), "seed {seed}: panic line of {test}");
            }
            receipt.absorb(found.failures);
        }
        let mine = receipt.failures.iter().filter(|each| each.job == "j1");
        assert_eq!(
            (receipt.failures.len(), mine.count()),
            (planted.len().saturating_mul(2), planted.len()),
            "seed {seed}: each of two jobs keeps its own copies"
        );
    }
    Ok(())
}

/// Source: run 45424270372. Fool: weird-bytes. Bug caught: a timestamp form, a
/// job and step column, a byte order mark or a colour spelling left in a line's
/// text, or text eaten with them.
#[test]
fn sim_ci_receipt_every_prefix_form_and_colour_spelling_cleans_to_the_text() -> SimResult {
    let alphabet = [
        'a', 'Z', '0', ' ', '\u{e9}', '\u{65e5}', '[', ']', '-', ':', '|', '.',
    ];
    let codes = [
        "^[[31;1m",
        "^[[0m",
        "\u{1b}[1m",
        "\u{1b}[38;5;9m",
        "\u{1b}[K",
        "^[[2K",
    ];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        for _ in 0..40 {
            let length = below(&mut draw, 40)?;
            let payload = word(&mut draw, &alphabet, length)?;
            let mut painted = String::new();
            for found in payload.chars() {
                if below(&mut draw, 6)? == 0 {
                    painted.push_str(pick(&mut draw, &codes)?);
                }
                painted.push(found);
            }
            let line = stamp(&mut draw, &painted)?;
            assert_eq!(clean(&line), payload, "seed {seed}: {line:?}");
        }
    }
    Ok(())
}

/// Source: run 45424270372. Fool: giant-and-empty. Bug caught: a bound that never
/// binds: 2,100 failed tests must keep 2,048 and say so, a million-character line
/// must keep 4,096, and a flood of `##[error]` lines must still give a bounded step
/// that keeps its newest errors, where the exit code is.
#[test]
fn sim_ci_receipt_a_giant_log_stays_bounded_and_says_so() -> SimResult {
    let log: String = (0..2_100_u32)
        .map(|number| format!("        FAIL [   0.1s] ({number}/9) p::it m::t{number}\n"))
        .collect();
    let found = extract("giant", &log);
    assert_eq!(found.failures.len(), MAX_HITS, "failures kept at the bound");
    assert!(found.shed, "a log past the bound says so");
    let line = "x".repeat(1_000_000);
    let kept = clean(&line).chars().count();
    assert_eq!(kept, MAX_LINE_CHARS, "a giant line is cut");
    let mut flood: Vec<String> = (0..1_000)
        .map(|number| format!("##[error]e{number}"))
        .collect();
    flood.push("##[error]Process completed with exit code 3.".to_owned());
    let words: Vec<&str> = flood.iter().map(String::as_str).collect();
    let step = extract("flood", &raw(&words));
    let only = step.failures.first().ok_or("a step failure")?;
    assert_eq!(only.kind, Kind::Step, "an error flood is one step failure");
    let message = only.message.as_str();
    assert!(message.chars().count() <= 400, "its message is bounded");
    assert!(message.ends_with("exit code 3."), "the newest: {message}");
    assert!(
        extract("empty", "").failures.is_empty(),
        "an empty log is no failure"
    );
    Ok(())
}

/// Source: run 45424270372. Fool: pulled-plug. Bug caught: a log past the read
/// bound read from its start instead of its end, a cut line kept as whole, or a
/// log under the bound marked clipped.
#[test]
fn sim_ci_receipt_the_tail_read_keeps_the_end_and_drops_the_cut_line() -> SimResult {
    let scratch = Fixture::claim(&std::env::temp_dir(), "sim-ci-receipt-tail")?;
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let count = below(&mut draw, 40)?.saturating_add(1);
        let mut lines = Vec::new();
        for number in 0..count {
            let width = below(&mut draw, 30)?;
            lines.push(format!("line {number} {}", "x".repeat(width)));
        }
        let body = format!("{}\n", lines.join("\n"));
        let path = scratch.path().join(format!("{seed}.log"));
        fs::write(&path, &body)?;
        let limit = u64::try_from(below(&mut draw, body.len().saturating_add(20))?)?;
        let read = logs::read_within(&path, limit)?;
        if limit >= u64::try_from(body.len())? {
            let whole = !read.clipped && read.text == body;
            assert!(whole, "seed {seed}: a log within the bound is whole");
            continue;
        }
        let before = body.get(..body.len().saturating_sub(read.text.len()));
        let at_line = read.text.is_empty() || before.is_some_and(|front| front.ends_with('\n'));
        assert!(read.clipped, "seed {seed}: a log past the bound says so");
        assert!(body.ends_with(&read.text), "seed {seed}: the end is kept");
        assert!(
            u64::try_from(read.text.len())? <= limit,
            "seed {seed}: bounded"
        );
        assert!(at_line, "seed {seed}: it starts at a line");
    }
    Ok(())
}

/// Lines that look like a seed and are not.
const DECOYS: [&str; 7] = [
    "reseed=5",
    "seed=0x2a",
    "seed=",
    "seedling 7",
    "seed42",
    "my_seed=9",
    "seed=12ab",
];

/// Source: run 45424270372. Fool: fat-finger. Bug caught: a seed read from a word
/// that only contains `seed`, from hex, or with its digits cut or run on.
#[test]
fn sim_ci_receipt_a_seed_is_the_digits_after_seed_and_a_separator() -> SimResult {
    let forms = ["seed=", "seed: ", "seed ", "seed = ", "seed:", " sim seed="];
    let tails = ["", " x", ",", ")", ".", "\t"];
    let digits = ['0', '1', '7', '9'];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        for _ in 0..20 {
            let length = below(&mut draw, 20)?.saturating_add(1);
            let number = word(&mut draw, &digits, length)?;
            let (text, wanted) = if below(&mut draw, 3)? == 0 {
                ((*pick(&mut draw, &DECOYS)?).to_owned(), None)
            } else {
                let form = *pick(&mut draw, &forms)?;
                let tail = *pick(&mut draw, &tails)?;
                (format!("{form}{number}{tail}"), Some(number))
            };
            let lines = vec![
                "        FAIL [   0.1s] ( 1/1) p::it m::t".to_owned(),
                "    noise".to_owned(),
                format!("    {text}"),
            ];
            let log = log_of(&mut draw, &lines)?;
            let failures = extract("seed", &log).failures;
            let hit = failures.into_iter().next().ok_or("a failure")?;
            assert_eq!(hit.seed, wanted, "seed {seed}: {text:?}");
        }
    }
    Ok(())
}

/// Source: run 45430022570. Fool: monkey. Bug caught: a diagnostic found past its
/// 6-line window, or lost inside it, one with a blank line before its place or
/// indented as test output taken for a real one, or the wrong code kept.
#[test]
fn sim_ci_receipt_a_diagnostic_is_found_exactly_when_its_place_is_in_the_window() -> SimResult {
    let files = ["src/lib.rs", "crates/a-b/src/x.rs", "a b/c.rs"];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        for _ in 0..20 {
            let gap = below(&mut draw, 10)?;
            let file = *pick(&mut draw, &files)?;
            let row = u32::try_from(below(&mut draw, 900)?)?.saturating_add(1);
            let coded = below(&mut draw, 2)? == 0;
            let indent = *pick(&mut draw, &["    ", "", "", ""])?;
            let blank = below(&mut draw, 4)? == 0;
            let (header, code) = if coded {
                ("error[E0425]: no value", "E0425")
            } else {
                ("error: no lint", "clippy::drawn_lint")
            };
            let mut lines = vec![format!("{indent}{header}")];
            lines.extend((0..gap).map(|number| format!("     filler {number}")));
            if blank {
                lines.push(String::new());
            }
            lines.push(format!("  --> {file}:{row}:5"));
            lines.push(
                "   = note: requested on the command line with `-D clippy::drawn_lint`".to_owned(),
            );
            let log = log_of(&mut draw, &lines)?;
            let found = extract("diag", &log).failures;
            let in_window = gap < 6 && !blank && indent.is_empty();
            let wanted = usize::from(in_window);
            assert_eq!(
                found.len(),
                wanted,
                "seed {seed}: gap {gap} blank {blank} {indent:?}"
            );
            if let Some(hit) = found.first() {
                let read = (hit.code.as_deref(), hit.file.as_deref(), hit.line);
                let place = (Some(code), Some(file), Some(row));
                assert_eq!(read, place, "seed {seed}: the code and the place");
            }
        }
    }
    Ok(())
}

/// Source: run 45424270372. Fool: wrong-order. Bug caught: the failing step taken
/// as the first or the last group of the log, instead of the group of the last
/// `Process completed` error, or of the last error when none says that.
#[test]
fn sim_ci_receipt_the_failing_step_is_the_one_of_the_last_completed_error() -> SimResult {
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let steps = below(&mut draw, 6)?.saturating_add(1);
        let mut lines = Vec::new();
        let mut model: Vec<(String, Vec<(String, bool)>)> = Vec::new();
        for index in 0..steps {
            let command = format!("run-{index} --flag");
            lines.push(format!("##[group]Run {command}"));
            lines.push("##[endgroup]".to_owned());
            let mut errors = Vec::new();
            for number in 0..below(&mut draw, 3)? {
                let completed = below(&mut draw, 2)? == 0;
                let text = if completed {
                    format!("Process completed with exit code {number}.")
                } else {
                    format!("annotation {index}-{number}")
                };
                lines.push(format!("##[error]{text}"));
                errors.push((text, completed));
            }
            model.push((command, errors));
        }
        let mut newest = model.iter().rev();
        let by_completion = newest
            .clone()
            .find(|step| step.1.iter().any(|error| error.1));
        let by_last = newest.find(|step| !step.1.is_empty());
        let log = log_of(&mut draw, &lines)?;
        let found = extract("steps", &log).failures;
        let Some(step) = by_completion.or(by_last) else {
            assert!(found.is_empty(), "seed {seed}: no error, no failure");
            continue;
        };
        let texts: Vec<&str> = step.1.iter().map(|error| error.0.as_str()).collect();
        let hit = found.first().ok_or("a step failure")?;
        let read = (hit.kind, hit.repro.as_str(), hit.message.clone());
        let wanted = (Kind::Step, step.0.as_str(), texts.join("\n"));
        assert_eq!(read, wanted, "seed {seed}: the step and its errors");
    }
    Ok(())
}
