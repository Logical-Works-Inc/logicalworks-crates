//! Unit tests of the `ci-receipt` extraction rules, the receipt bounds, the
//! renderers and the command line, on the real fixture logs and synthetic ones.
//!
//! Sources: `linux-slice-4.log` is run 45424270372 and `linux-slice-7.log` is run
//! 45430022570, both real `gh run view --log` output; `compile-error.log` and
//! `step-only.log` are synthetic. The seeded simulations are in `sim_ci_receipt`.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;

use super::error::ReceiptError;
use super::extract::{extract, test_repro};
use super::failure::{Failure, Kind, MAX_HITS, MESSAGE_CHARS, Receipt};
use super::fixture::Fixture;
use super::{execute, logs, parse_args, render};

/// The result a test returns instead of unwrapping.
type TestResult = Result<(), Box<dyn Error>>;

/// Run 45424270372: an orphan the test left behind.
const SLICE_4: &str = include_str!("../../../tests/fixtures/ci_receipt/linux-slice-4.log");

/// Run 45430022570: a failed assertion.
const SLICE_7: &str = include_str!("../../../tests/fixtures/ci_receipt/linux-slice-7.log");

/// A synthetic log with one `error[E0425]`.
const COMPILE: &str = include_str!("../../../tests/fixtures/ci_receipt/compile-error.log");

/// A synthetic log with only a failed step.
const STEP_ONLY: &str = include_str!("../../../tests/fixtures/ci_receipt/step-only.log");

/// The `NEXTEST_BUILD` flags of both real runs.
const FLAGS: &str = "--workspace --exclude lgwks_bot --locked --cargo-profile ci-bot";

/// `lines` as a raw API log: each behind a timestamp and a space.
pub(crate) fn raw(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|text| format!("2026-11-24T16:00:00.0000000Z {text}\n"))
        .collect()
}

/// A test failure of `job` at `file`:`line`, with the rest plain.
pub(crate) fn failure(job: &str, file: Option<&str>, line: Option<u32>) -> Failure {
    Failure {
        job: job.to_owned(),
        kind: Kind::Test,
        test: Some("t".to_owned()),
        binary: None,
        file: file.map(str::to_owned),
        line,
        code: None,
        seed: None,
        message: String::new(),
        repro: String::new(),
        step: String::new(),
    }
}

/// A log that was expected to hold a failure and held none.
#[derive(Debug)]
pub(crate) struct NoFailure;

impl fmt::Display for NoFailure {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("the log holds no failure")
    }
}

impl Error for NoFailure {}

/// The first failure of `log`.
fn first(job: &str, log: &str) -> Result<Failure, NoFailure> {
    extract(job, log)
        .failures
        .into_iter()
        .next()
        .ok_or(NoFailure)
}

/// The receipt of `logs` read in the order `order` names, as JSON.
fn build(logs: &[(&str, &str)], order: [usize; 3]) -> Result<String, ReceiptError> {
    let mut receipt = Receipt::new(50);
    for entry in order.iter().filter_map(|at| logs.get(*at)) {
        receipt.absorb(extract(entry.0, entry.1).failures);
    }
    render::json(&receipt)
}

/// Source: run 45424270372. Requirement: a nextest `FAIL` line gives the binary,
/// the test and the repro; a `churn-slot` line is the message; the `Summary`
/// repeat of the same failure is not a second one.
#[test]
fn a_nextest_fail_line_gives_the_test_and_the_orphan_message() -> TestResult {
    let found = extract("linux-slice-4", SLICE_4);
    assert_eq!(
        found.failures.len(),
        1,
        "the summary repeat is no second failure"
    );
    let hit = first("linux-slice-4", SLICE_4)?;
    let name = "sim_scan_examples::the_shipped_scan_holds_a_change_to_its_added_lines";
    assert_eq!(hit.kind, Kind::Test, "a test failure");
    assert_eq!(
        hit.binary.as_deref(),
        Some("lgwks_ast::it"),
        "the binary id"
    );
    assert_eq!(hit.test.as_deref(), Some(name), "the test name");
    assert_eq!(
        (hit.file, hit.line),
        (None, None),
        "an orphan names no panic site"
    );
    assert!(
        hit.message
            .contains("churn-slot: orphans: the test left 1 live processes: 124916 (git)"),
        "the orphan line, colour gone: {}",
        hit.message
    );
    assert!(
        !hit.message.contains('\u{1b}') && !hit.message.contains("^["),
        "no colour left"
    );
    assert_eq!(
        hit.repro,
        format!("cargo nextest run {FLAGS} -E 'test(={name})'"),
        "the repro uses the log's NEXTEST_BUILD"
    );
    assert!(
        hit.step
            .starts_with("cargo nextest run ${NEXTEST_BUILD:---workspace --locked}"),
        "the failing step is the group's first line: {}",
        hit.step
    );
    Ok(())
}

/// Source: run 45430022570. Requirement: the first `panicked at <file>:<line>:<col>:`
/// gives the file and line, and the message is the text under it, stopping at the
/// `note:` line.
#[test]
fn a_panic_gives_the_file_the_line_and_the_assertion_lines() -> TestResult {
    let hit = first("linux-slice-7", SLICE_7)?;
    assert_eq!(
        hit.file.as_deref(),
        Some("crates/lgwks-deps/src/bin/check_gate_parity/workflow_calls.rs"),
        "the panic's file"
    );
    assert_eq!(hit.line, Some(597), "the panic's line");
    assert!(
        hit.message.starts_with("assertion `left == right` failed"),
        "the message opens with the assertion: {}",
        hit.message
    );
    assert!(
        hit.message.contains("hosted-rust's base build"),
        "and holds the left side: {}",
        hit.message
    );
    assert!(
        hit.message.contains("right: []"),
        "and the right side: {}",
        hit.message
    );
    assert_eq!(
        hit.binary.as_deref(),
        Some("lgwks_deps::bin/check-gate-parity"),
        "binary id"
    );
    Ok(())
}

/// Requirement: a compile error gives its code, file, line and text, and a clippy
/// error gives the lint it was raised under, from either note spelling; a header
/// with no place within 6 lines is no failure.
#[test]
fn a_diagnostic_gives_its_code_place_and_lint() -> TestResult {
    let hit = first("compile-error", COMPILE)?;
    assert_eq!(hit.kind, Kind::Compile, "a compile failure");
    assert_eq!(hit.code.as_deref(), Some("E0425"), "the bracketed code");
    assert_eq!(hit.file.as_deref(), Some("crates/x/src/lib.rs"), "the file");
    assert_eq!(hit.line, Some(3), "the line");
    assert_eq!(
        hit.message, "cannot find value `total` in this scope",
        "the text"
    );
    assert_eq!(
        hit.repro, "cargo clippy --locked -p x --all-targets -- -D warnings",
        "the failing step's command"
    );
    let lint = raw(&[
        "error: used `unwrap()` on an `Option` value",
        "  --> crates/x/src/main.rs:10:9",
        "   |",
        "   = note: requested on the command line with `-D clippy::unwrap_used`",
        "",
        "error: used `expect()` on a `Result` value",
        " --> crates/x/src/main.rs:20:9",
        "   = note: `#[deny(clippy::expect_used)]` on by default",
        "",
        "error: too far",
        "a",
        "b",
        "c",
        "d",
        "e",
        "f",
        " --> crates/x/src/main.rs:30:9",
    ]);
    let lints: Vec<(Option<String>, Option<u32>)> = extract("lints", &lint)
        .failures
        .into_iter()
        .map(|found| (found.code, found.line))
        .collect();
    assert_eq!(
        lints,
        vec![
            (Some("clippy::unwrap_used".to_owned()), Some(10)),
            (Some("clippy::expect_used".to_owned()), Some(20)),
        ],
        "two lints, and the header past the window gives none"
    );
    Ok(())
}

/// Requirement: a `step` failure is the last `##[group]Run` before the
/// `Process completed` error, and it is reported only when no test or diagnostic
/// was found.
#[test]
fn a_step_is_reported_only_when_nothing_more_specific_is_found() -> TestResult {
    let hit = first("step-only", STEP_ONLY)?;
    assert_eq!(hit.kind, Kind::Step, "a step failure");
    assert_eq!(
        hit.message, "docs lane refused 1 page\nProcess completed with exit code 1.",
        "the step's ##[error] lines"
    );
    assert_eq!(
        hit.repro, "./scripts/check-docs.sh --base origin/main",
        "the command"
    );
    assert_eq!(
        (hit.file, hit.line, hit.test),
        (None, None, None),
        "no place"
    );
    let two = raw(&[
        "##[group]Run echo first",
        "##[endgroup]",
        "##[group]Run make second",
        "##[endgroup]",
        "##[error]Process completed with exit code 2.",
        "##[group]Run echo cleanup",
    ]);
    assert_eq!(
        first("two", &two)?.repro,
        "make second",
        "the group before the error"
    );
    for log in [SLICE_4, SLICE_7, COMPILE] {
        let kinds: Vec<Kind> = extract("j", log)
            .failures
            .iter()
            .map(|found| found.kind)
            .collect();
        assert!(
            !kinds.contains(&Kind::Step),
            "a specific failure hides the step: {kinds:?}"
        );
    }
    Ok(())
}

/// Requirement: an empty or malformed log gives no failure, and a log with an
/// `##[error]` but nothing else gives the step.
#[test]
fn an_empty_or_malformed_log_gives_no_failure() -> TestResult {
    for log in [
        "",
        "\n\n",
        "garbage\u{0}\u{1b}[\n^[[\n\t\t\n",
        "FAIL [",
        "error: x\n--> :",
    ] {
        assert!(
            extract("j", log).failures.is_empty(),
            "no failure in {log:?}"
        );
    }
    let bare = first("bare", &raw(&["##[error]The operation was canceled."]))?;
    assert_eq!(bare.kind, Kind::Step, "an error alone is a step failure");
    assert_eq!(
        (bare.step.as_str(), bare.repro.as_str()),
        ("", ""),
        "with no command to name"
    );
    Ok(())
}

/// Requirement: the repro uses the `NEXTEST_BUILD` flags when the log has them,
/// else the package before the first `::`, and writes none for a name that is not
/// safe inside quotes.
#[test]
fn the_repro_follows_the_flags_else_the_package_and_refuses_unsafe_names() {
    let test = "a::b";
    assert_eq!(
        test_repro(Some("--workspace --locked"), "p::it", test),
        "cargo nextest run --workspace --locked -E 'test(=a::b)'",
        "with flags"
    );
    assert_eq!(
        test_repro(None, "lgwks_deps::bin/check-gate-parity", test),
        "cargo nextest run --locked -p lgwks_deps -E 'test(=a::b)'",
        "the package of a bin binary id"
    );
    assert_eq!(
        test_repro(None, "lgwks_ast", test),
        "cargo nextest run --locked -p lgwks_ast -E 'test(=a::b)'",
        "a binary id with no `::` is the package"
    );
    assert_eq!(
        test_repro(Some("--a; rm -rf ~"), "p::it", test),
        "cargo nextest run --locked -p p -E 'test(=a::b)'",
        "unsafe flags fall back to the package"
    );
    for name in ["x'; rm -rf ~ #", "a b", "a`b`", "$(x)", "a)b", ""] {
        assert_eq!(test_repro(None, "p::it", name), "", "no repro for {name:?}");
    }
    assert_eq!(
        test_repro(None, "p q", test),
        "",
        "no repro for an unsafe package"
    );
    let widest = "a".repeat(MESSAGE_CHARS);
    assert_eq!(
        test_repro(None, "p::it", &widest),
        format!("cargo nextest run --locked -p p -E 'test(={widest})'"),
        "a name of exactly the bound has a repro"
    );
    let past = "a".repeat(MESSAGE_CHARS.saturating_add(1));
    assert_eq!(test_repro(None, "p::it", &past), "", "one past it has none");
}

/// Source: nextest prints `(test <status>)` under a test that failed with no
/// panic, for example on a signal. Requirement: that trailer is the message, and
/// an output line that only ends in `)` or only opens with `(test ` is not it.
#[test]
fn the_status_trailer_is_the_message_of_a_failure_with_no_panic() -> TestResult {
    let log = raw(&[
        "        FAIL [   0.1s] ( 1/1) p::it m::t",
        "    called close(fd)",
        "    (test harness started",
        "    (test aborted with signal 6: SIGABRT)",
    ]);
    assert_eq!(
        first("trailer", &log)?.message,
        "test aborted with signal 6: SIGABRT",
        "the trailer, not a line that only looks like one"
    );
    Ok(())
}

/// Source: the replay harness prints `seed = <n>`, `seed: <n>` or `seed <n>`.
/// Requirement: the seed is the first `seed` word with a short number after it; a
/// longer word (`reseed`, `seedless`), no number, a number past `u64` or one glued
/// to letters is skipped, and the search goes on to the next `seed`.
#[test]
fn the_seed_is_the_first_whole_seed_word_with_a_number() -> TestResult {
    let cases = [
        ("reseed=5 then seed=7", Some("7")),
        ("seedless run, seed: 9", Some("9")),
        ("seed: none, seed 4", Some("4")),
        ("seed 123456789012345678901 seed 8", Some("8")),
        ("seed 12abc seed 6", Some("6")),
        ("seedseed 3", None),
        ("_seed 3", None),
        ("no number at all", None),
    ];
    for (at, case) in cases.iter().enumerate() {
        let log = raw(&[
            &format!("        FAIL [   0.1s] ( 1/1) p::it m::t{at}"),
            case.0,
        ]);
        assert_eq!(
            first("seed", &log)?.seed.as_deref(),
            case.1,
            "the seed in {:?}",
            case.0
        );
    }
    Ok(())
}

/// Source: a GitHub job log reads at most `MAX_LOG_BYTES`, and one log can hold
/// more failures than `MAX_HITS`. Requirement: a receipt that dropped text from
/// one log says so, even when the next log read is whole.
#[test]
fn a_flooded_log_marks_the_receipt_though_a_clean_log_follows() -> TestResult {
    let scratch = Fixture::claim(&std::env::temp_dir(), "ci-receipt-flood")?;
    let dir = scratch.path();
    let flood: Vec<String> = (0..=MAX_HITS)
        .map(|at| format!("        FAIL [   0.1s] ( 1/1) p::it m::t{at}"))
        .collect();
    let lines: Vec<&str> = flood.iter().map(String::as_str).collect();
    fs::write(dir.join("a-flood.log"), raw(&lines))?;
    fs::write(dir.join("b-clean.log"), SLICE_7)?;
    let out = dir.join("receipt.json");
    let table = dir.join("receipt.md");
    let mut line = words(&["--logs"]);
    line.push(dir.as_os_str().to_owned());
    line.push(OsString::from("--out"));
    line.push(out.as_os_str().to_owned());
    line.push(OsString::from("--markdown"));
    line.push(table.as_os_str().to_owned());
    execute(&line)?;
    let markdown = fs::read_to_string(&table)?;
    assert!(
        markdown.contains("Some log text lay past a read bound and was not read."),
        "the shed survives the clean log after it: {markdown}"
    );
    Ok(())
}

/// Requirement: a message is cut at 400 characters on a character boundary: 399
/// ASCII characters and two of 3 bytes keep 400 characters and 402 bytes.
#[test]
fn a_message_is_cut_at_400_chars_on_a_char_boundary() -> TestResult {
    let long = format!("{}\u{65e5}\u{65e5}", "a".repeat(399));
    let log = raw(&[
        "        FAIL [   0.1s] ( 1/1) p::it m::t",
        "    thread 'm::t' panicked at src/x.rs:1:1:",
        &long,
    ]);
    let hit = first("cut", &log)?;
    assert_eq!(hit.message.chars().count(), 400, "400 characters");
    assert!(
        hit.message.ends_with('\u{65e5}'),
        "the last kept character is whole"
    );
    assert_eq!(
        hit.message.len(),
        399 + 3,
        "399 one-byte characters and one of three"
    );
    Ok(())
}

/// Requirement: `--max` keeps the first failures in job, file, line order, counts
/// the rest as cut and flags it; the markdown says how many.
#[test]
fn the_max_cuts_the_receipt_and_both_renderings_say_so() -> TestResult {
    let mut receipt = Receipt::new(2);
    receipt.absorb(vec![
        failure("b", Some("f.rs"), Some(1)),
        failure("a", Some("z.rs"), Some(9)),
    ]);
    receipt.absorb(vec![
        failure("a", Some("a.rs"), Some(5)),
        failure("c", None, None),
    ]);
    let kept: Vec<(&str, Option<u32>)> = receipt
        .failures
        .iter()
        .map(|found| (found.job.as_str(), found.line))
        .collect();
    assert_eq!(
        kept,
        vec![("a", Some(5)), ("a", Some(9))],
        "the first two in order"
    );
    assert_eq!(
        (receipt.cut, receipt.truncated()),
        (2, true),
        "two cut, flagged"
    );
    let table = render::markdown(&receipt, 2);
    assert!(
        table.contains("Truncated at --max 2; failures cut: 2.\n"),
        "{table}"
    );
    assert!(table.contains("| a | test | a.rs:5 | t | - |\n"), "{table}");
    let json = render::json(&receipt)?;
    assert!(
        json.starts_with("{\"schema\":1,\"failures\":[{\"job\":\"a\",\"kind\":\"test\","),
        "{json}"
    );
    assert!(json.ends_with("],\"truncated\":true}\n"), "{json}");
    let none = render::markdown(&Receipt::new(5), 5);
    assert_eq!(
        none, "### CI failure receipt\n\nNo failure was found in the logs.\n",
        "empty"
    );
    Ok(())
}

/// Requirement: a cell holds no `|`, no newline and no live mention, and a repro
/// with backticks gets a longer fence.
#[test]
fn a_markdown_cell_is_escaped_and_a_repro_is_fenced() {
    let mut hostile = failure("job|@octocat", Some("a|b.rs"), Some(1));
    hostile.repro = "echo `x` | cat".to_owned();
    let mut receipt = Receipt::new(5);
    receipt.absorb(vec![hostile]);
    let table = render::markdown(&receipt, 5);
    assert!(
        table.contains("job\\|@\u{200b}octocat"),
        "pipe escaped, mention broken: {table}"
    );
    assert!(table.contains("a\\|b.rs:1"), "{table}");
    assert!(
        table.contains("`` echo `x` \\| cat ``"),
        "fenced by two backticks: {table}"
    );
}

/// Requirement: the same logs give the same bytes, read in any order, with the
/// keys in the schema's order.
#[test]
fn the_same_logs_give_the_same_bytes_in_any_order() -> TestResult {
    let logs = [
        ("linux-slice-4", SLICE_4),
        ("linux-slice-7", SLICE_7),
        ("compile-error", COMPILE),
    ];
    let forward = build(&logs, [0, 1, 2])?;
    assert_eq!(
        forward,
        build(&logs, [2, 1, 0])?,
        "the order of reading changes nothing"
    );
    assert_eq!(
        forward,
        build(&logs, [0, 1, 2])?,
        "and a second run is identical"
    );
    assert!(
        forward.starts_with("{\"schema\":1,\"failures\":[{\"job\":\"compile-error\",\"kind\":\"compile\",\"test\":null,\"binary\":null,\"file\":\"crates/x/src/lib.rs\",\"line\":3,\"code\":\"E0425\",\"seed\":null,\"message\":"),
        "{forward}"
    );
    Ok(())
}

/// A command line of `list`.
fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// Requirement: each bad command line is refused as its own variant, never
/// guessed at.
#[test]
fn the_command_line_refuses_each_bad_shape_by_its_variant() -> TestResult {
    let refused = |line: &[&str]| parse_args(&words(line)).err();
    assert!(
        matches!(refused(&[]), Some(ReceiptError::MissingFlag("--logs"))),
        "no --logs"
    );
    assert!(
        matches!(
            refused(&["--logs", "a"]),
            Some(ReceiptError::MissingFlag("--out"))
        ),
        "no --out"
    );
    assert!(
        matches!(
            refused(&["--logs"]),
            Some(ReceiptError::MissingValue("--logs"))
        ),
        "no value"
    );
    assert!(
        matches!(
            refused(&["--logs", "--out", "x"]),
            Some(ReceiptError::MissingValue("--logs"))
        ),
        "a flag as a value"
    );
    assert!(
        matches!(
            refused(&["--logs", "a", "--logs", "b"]),
            Some(ReceiptError::DuplicateFlag("--logs"))
        ),
        "twice"
    );
    assert!(
        matches!(
            refused(&["--bogus"]),
            Some(ReceiptError::UnknownArgument(_))
        ),
        "unknown"
    );
    for bad in ["0", "101", "x", "-1", "", "1.5"] {
        let line = ["--logs", "a", "--out", "b", "--max", bad];
        assert!(
            matches!(refused(&line), Some(ReceiptError::BadMax(_))),
            "--max {bad:?}"
        );
    }
    let args = parse_args(&words(&[
        "--logs",
        "a",
        "--out",
        "b",
        "--markdown",
        "m",
        "--max",
        "100",
    ]))?;
    assert_eq!(
        (args.max, args.markdown.is_some()),
        (100, true),
        "the edge and the optional flag"
    );
    let default = parse_args(&words(&["--out", "b", "--logs", "a"]))?;
    assert_eq!(default.max, 50, "the default is 50");
    Ok(())
}

/// Requirement: only regular `.log` files are logs, by name; a missing directory
/// and a directory with too many logs are refused as their own variants.
#[test]
fn a_directory_lists_only_log_files_and_refuses_the_rest_by_variant() -> TestResult {
    let scratch = Fixture::claim(&std::env::temp_dir(), "ci-receipt-list")?;
    let dir = scratch.path();
    for name in ["b.log", "a.log", "c.txt", ".log", "d.LOG", "e.log.txt"] {
        fs::write(dir.join(name), "x")?;
    }
    // Many of each, so the directory's own order puts a non-log before a log.
    let mut want = vec!["a".to_owned(), "b".to_owned()];
    for at in 0..16 {
        fs::write(dir.join(format!("m{at:02}.log")), "x")?;
        fs::write(dir.join(format!("n{at:02}.txt")), "x")?;
        want.push(format!("m{at:02}"));
    }
    fs::create_dir(dir.join("dir.log"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.join("a.log"), dir.join("link.log"))?;
    let jobs: Vec<String> = logs::list(dir)?.into_iter().map(|file| file.job).collect();
    assert_eq!(jobs, want, "regular .log files by name");
    let absent = logs::list(&dir.join("absent")).err();
    assert!(
        matches!(absent, Some(ReceiptError::LogsDir { .. })),
        "a missing directory"
    );
    let crowd = Fixture::claim(&std::env::temp_dir(), "ci-receipt-crowd")?;
    for number in 0..=logs::MAX_LOGS {
        fs::write(crowd.path().join(format!("{number}.log")), "")?;
    }
    let refused = logs::list(crowd.path()).err();
    assert!(
        matches!(refused, Some(ReceiptError::TooManyLogs { .. })),
        "513 logs"
    );
    Ok(())
}
