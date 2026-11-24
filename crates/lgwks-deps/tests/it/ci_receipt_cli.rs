//! The shipped `ci-receipt` binary turns failed-job logs into a receipt, driven on
//! a scratch copy of the fixture logs.
//!
//! Source: `linux-slice-4.log` is run 45424270372 (an orphan the test left behind,
//! reported by `churn-slot`) and `linux-slice-7.log` is run 45430022570 (a failed
//! assertion), both real `gh run view --log` output trimmed to the lines the reader
//! needs. `compile-error.log` and `step-only.log` are synthetic, in the raw Actions
//! API form. Requirement proved: an agent reproduces a CI failure locally from the
//! receipt, without reading the raw log.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lgwks_std::json::{self, Value};

use crate::lock_fixture::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// What nextest ran in the real runs, from their `NEXTEST_BUILD` line.
const FLAGS: &str = "--workspace --exclude lgwks_bot --locked --cargo-profile ci-bot";

/// Copies the fixture logs into `<scratch>/logs` and returns that directory.
fn logs_copy(scratch: &Scratch) -> Result<PathBuf, Box<dyn Error>> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ci_receipt");
    let target = scratch.path().join("logs");
    fs::create_dir_all(&target)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        fs::copy(entry.path(), target.join(entry.file_name()))?;
    }
    Ok(target)
}

/// Runs the shipped binary with `arguments`.
fn run(arguments: &[&Path]) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_ci-receipt"))
        .args(arguments)
        .output()?)
}

/// `--logs <logs> --out <out>`, then the extra flags in `more`.
fn receipt_run(logs: &Path, out: &Path, more: &[&Path]) -> Result<Output, Box<dyn Error>> {
    let mut arguments = vec![Path::new("--logs"), logs, Path::new("--out"), out];
    arguments.extend_from_slice(more);
    run(&arguments)
}

/// What the binary said on standard error.
fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The receipt written to `out`, parsed.
fn parsed(out: &Path) -> Result<Value, Box<dyn Error>> {
    Ok(json::from_str::<Value>(&fs::read_to_string(out)?)?)
}

/// The string under `key`, if the member is one.
fn text<'value>(value: &'value Value, key: &str) -> Option<&'value str> {
    value.get(key)?.as_str()
}

/// The failures array of a receipt.
fn failures_of(receipt: &Value) -> Result<&Vec<Value>, Box<dyn Error>> {
    Ok(receipt
        .get("failures")
        .and_then(Value::as_array)
        .ok_or("the receipt holds a failures array")?)
}

/// One failure's fields as the test expects to read them back.
struct Expected {
    /// The log's file stem.
    job: &'static str,
    /// `test`, `compile` or `step`.
    kind: &'static str,
    /// The test's name.
    test: Option<&'static str>,
    /// The file the failure names.
    file: Option<&'static str>,
    /// The line the failure names.
    line: Option<u64>,
    /// The command that reproduces it.
    repro: String,
}

/// The four failures the fixtures hold, in the receipt's order.
fn expected() -> [Expected; 4] {
    let nextest = |test: &str| format!("cargo nextest run {FLAGS} -E 'test(={test})'");
    let orphan = "sim_scan_examples::the_shipped_scan_holds_a_change_to_its_added_lines";
    let moved = "workflow_calls::tests::a_moved_lockfile_restores_mains_nearest_build";
    [
        Expected {
            job: "compile-error",
            kind: "compile",
            test: None,
            file: Some("crates/x/src/lib.rs"),
            line: Some(3),
            repro: "cargo clippy --locked -p x --all-targets -- -D warnings".to_owned(),
        },
        Expected {
            job: "linux-slice-4",
            kind: "test",
            test: Some(orphan),
            file: None,
            line: None,
            repro: nextest(orphan),
        },
        Expected {
            job: "linux-slice-7",
            kind: "test",
            test: Some(moved),
            file: Some("crates/lgwks-deps/src/bin/check_gate_parity/workflow_calls.rs"),
            line: Some(597),
            repro: nextest(moved),
        },
        Expected {
            job: "step-only",
            kind: "step",
            test: None,
            file: None,
            line: None,
            repro: "./scripts/check-docs.sh --base origin/main".to_owned(),
        },
    ]
}

/// Source: runs 45424270372 and 45430022570. Requirement: a receipt names, for each
/// failed job, the kind, the place and the exact command that runs it again.
#[test]
fn the_real_failures_become_a_receipt() -> TestResult {
    let scratch = Scratch::new("ci-receipt-real")?;
    let logs = logs_copy(&scratch)?;
    let out = scratch.path().join("receipt.json");
    let output = receipt_run(&logs, &out, &[])?;
    assert_eq!(output.status.code(), Some(0), "stderr: {}", said(&output));
    let receipt = parsed(&out)?;
    assert_eq!(
        receipt.get("schema").and_then(Value::as_u64),
        Some(1),
        "the schema is 1"
    );
    assert_eq!(
        receipt.get("truncated").and_then(Value::as_bool),
        Some(false),
        "nothing is cut"
    );
    let failures = failures_of(&receipt)?;
    let wanted = expected();
    assert_eq!(failures.len(), wanted.len(), "one failure per fixture log");
    for (found, want) in failures.iter().zip(&wanted) {
        let job = want.job;
        assert_eq!(text(found, "job"), Some(job), "{job}: job");
        assert_eq!(text(found, "kind"), Some(want.kind), "{job}: kind");
        assert_eq!(text(found, "test"), want.test, "{job}: test");
        assert_eq!(text(found, "file"), want.file, "{job}: file");
        let line = found.get("line").and_then(Value::as_u64);
        assert_eq!(line, want.line, "{job}: line");
        assert_eq!(
            text(found, "repro"),
            Some(want.repro.as_str()),
            "{job}: repro"
        );
    }
    let orphan = failures.get(1).ok_or("the orphan failure")?;
    assert_eq!(
        text(orphan, "binary"),
        Some("lgwks_ast::it"),
        "the binary is the nextest binary id"
    );
    let message = text(orphan, "message").ok_or("an orphan message")?;
    assert!(
        message.contains("churn-slot: orphans: the test left 1 live processes: 124916 (git)"),
        "the orphan line is the message: {message}"
    );
    let moved = failures.get(2).ok_or("the assertion failure")?;
    let message = text(moved, "message").ok_or("an assertion message")?;
    assert!(
        message.contains("left == right") && message.contains("right: []"),
        "the assertion text is the message: {message}"
    );
    Ok(())
}

/// Source: run 45430022570. Requirement: the same logs give a byte-identical
/// receipt every time, so a receipt can be compared and replayed.
#[test]
fn the_same_logs_give_the_same_bytes() -> TestResult {
    let scratch = Scratch::new("ci-receipt-twice")?;
    let logs = logs_copy(&scratch)?;
    let first = scratch.path().join("first.json");
    let second = scratch.path().join("second.json");
    assert_eq!(
        receipt_run(&logs, &first, &[])?.status.code(),
        Some(0),
        "first run"
    );
    assert_eq!(
        receipt_run(&logs, &second, &[])?.status.code(),
        Some(0),
        "second run"
    );
    assert_eq!(
        fs::read(&first)?,
        fs::read(&second)?,
        "byte-identical receipts"
    );
    Ok(())
}

/// Source: run 45424270372. Requirement: `--max` cuts the failures, says so in the
/// receipt, and `--markdown` renders the same failures as a table with the count cut.
#[test]
fn a_max_cuts_the_receipt_and_the_markdown_says_so() -> TestResult {
    let scratch = Scratch::new("ci-receipt-max")?;
    let logs = logs_copy(&scratch)?;
    let out = scratch.path().join("receipt.json");
    let table = scratch.path().join("receipt.md");
    let more = [
        Path::new("--max"),
        Path::new("1"),
        Path::new("--markdown"),
        table.as_path(),
    ];
    let output = receipt_run(&logs, &out, &more)?;
    assert_eq!(output.status.code(), Some(0), "stderr: {}", said(&output));
    let receipt = parsed(&out)?;
    assert_eq!(
        receipt.get("truncated").and_then(Value::as_bool),
        Some(true),
        "the cut is flagged"
    );
    assert_eq!(failures_of(&receipt)?.len(), 1, "--max 1 keeps one failure");
    let markdown = fs::read_to_string(&table)?;
    assert!(
        markdown.starts_with("### CI failure receipt\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| compile-error | compile | crates/x/src/lib.rs:3 | E0425 |"),
        "the kept row: {markdown}"
    );
    assert!(
        markdown.contains("`cargo clippy --locked -p x --all-targets -- -D warnings`"),
        "the repro is in backticks: {markdown}"
    );
    assert!(
        markdown.contains("Truncated at --max 1; failures cut: 3."),
        "the count cut: {markdown}"
    );
    Ok(())
}

/// Requirement: a directory that is not there is a refusal with exit 2, and no
/// receipt is written.
#[test]
fn a_missing_logs_directory_exits_two_and_writes_nothing() -> TestResult {
    let scratch = Scratch::new("ci-receipt-missing")?;
    let absent = scratch.path().join("absent");
    let out = scratch.path().join("receipt.json");
    let output = receipt_run(&absent, &out, &[])?;
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit 2 on a missing directory"
    );
    assert!(
        said(&output).starts_with("ci-receipt: "),
        "{}",
        said(&output)
    );
    assert!(!out.exists(), "no receipt is written on a refusal");
    Ok(())
}

/// Source: run 45467652939. `bot-it-surface, slice:3/4` was cancelled at its
/// 5-minute limit after every step passed, its log held no failure line, and the
/// receipt left it out. Requirement: every failed job's log is at least one row,
/// a quiet one a `step` row that says no line named the failure.
#[test]
fn a_failed_job_whose_log_names_no_failure_is_still_a_row() -> TestResult {
    let scratch = Scratch::new("ci-receipt-quiet")?;
    let logs = logs_copy(&scratch)?;
    fs::write(
        logs.join("quiet.log"),
        "2026-11-24T19:17:05.4076146Z Finished uploading artifact content to blob storage!\n",
    )?;
    let out = scratch.path().join("receipt.json");
    let output = receipt_run(&logs, &out, &[])?;
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    let receipt = parsed(&out)?;
    let quiet: Vec<&Value> = failures_of(&receipt)?
        .iter()
        .filter(|found| text(found, "job") == Some("quiet"))
        .collect();
    assert_eq!(quiet.len(), 1, "one row for the quiet job");
    let row = quiet.first().ok_or("the quiet row")?;
    assert_eq!(text(row, "kind"), Some("step"), "a step row");
    assert!(
        text(row, "message")
            .is_some_and(|said| said.contains("no line of the log names the failure")),
        "that says why: {row:?}"
    );
    assert_eq!(
        failures_of(&receipt)?.len(),
        expected().len().saturating_add(1),
        "the fixtures' rows are unchanged"
    );
    Ok(())
}

/// Requirement: a flag that is not one is a refusal with exit 2, never a guess.
#[test]
fn an_unknown_flag_exits_two() -> TestResult {
    let output = run(&[
        Path::new("--logs"),
        Path::new("."),
        Path::new("--frobnicate"),
    ])?;
    assert_eq!(output.status.code(), Some(2), "exit 2 on bad arguments");
    assert!(said(&output).contains("--frobnicate"), "{}", said(&output));
    Ok(())
}
