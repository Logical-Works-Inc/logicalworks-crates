//! The built `lgwks-bench-durable` binary, run as a user runs it: a bad command
//! line or a missing path ends non-zero with one typed line on stderr and nothing
//! on stdout, and `--json` prints one document carrying the same numbers as the
//! human report.

use std::process::{Command, Output};

use lgwks_std::json::Value;

#[path = "../../crates/lgwks-bot/tests/support/scratch.rs"]
mod scratch;

/// A test's result.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The binary with `args`.
fn run(args: &[&str]) -> Result<Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_lgwks-bench-durable"))
        .args(args)
        .output()
}

/// The refusal contract: non-zero exit, empty stdout, one stderr line leading
/// with the program name and the error's type.
fn assert_refused(out: &Output, kind: &str) -> TestResult {
    let stderr = String::from_utf8(out.stderr.clone())?;
    assert!(!out.status.success(), "it exited 0: {stderr}");
    assert!(out.stdout.is_empty(), "stdout held {:?}", out.stdout);
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
    let want = format!("lgwks-bench-durable: {kind}: ");
    assert!(
        stderr.starts_with(&want),
        "{stderr:?} does not start with {want:?}"
    );
    Ok(())
}

/// The text report's `key: value` rows.
fn row<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}: ");
    text.lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

#[test]
fn a_malformed_number_is_refused_with_one_typed_line() -> TestResult {
    let out = run(&["--runs", "abc"])?;
    assert_eq!(out.status.code(), Some(2));
    assert_refused(&out, "MalformedArgument")
}

#[test]
fn an_unknown_flag_is_refused_with_one_typed_line() -> TestResult {
    let out = run(&["--fast"])?;
    assert_eq!(out.status.code(), Some(2));
    assert_refused(&out, "UnknownArgument")
}

#[test]
fn a_missing_work_directory_is_refused_with_one_typed_line() -> TestResult {
    let scratch = scratch::Scratch::new("durable-cli-missing")?;
    let missing = scratch.path().join("not").join("here");
    let missing = missing.to_str().ok_or("the path is not UTF-8")?;
    let out = run(&["--runs", "5", "--work-dir", missing])?;
    assert_eq!(out.status.code(), Some(2));
    assert_refused(&out, "UnreadablePath")?;
    assert!(
        !scratch.path().join("not").exists(),
        "the run made the missing path"
    );
    Ok(())
}

#[test]
fn json_is_one_document_with_the_percentiles_and_the_memory() -> TestResult {
    let scratch = scratch::Scratch::new("durable-cli-json")?;
    let dir = scratch.path().to_str().ok_or("the path is not UTF-8")?;
    let out = run(&["--runs", "40", "--seed", "3", "--json", "--work-dir", dir])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout.lines().count(), 1, "{stdout:?}");
    let doc = lgwks_std::json::from_str::<Value>(&stdout)?;
    let latency = &doc["latency_us"];
    let (p50, p95, p99) = (
        latency["p50"].as_u64().ok_or("no p50")?,
        latency["p95"].as_u64().ok_or("no p95")?,
        latency["p99"].as_u64().ok_or("no p99")?,
    );
    assert!(0 < p50 && p50 <= p95 && p95 <= p99, "{p50} {p95} {p99}");
    assert!(doc["peak_rss_bytes"].as_u64().ok_or("no rss")? > 0);
    let samples = doc["samples"].as_u64().ok_or("no samples")?;
    let given_up = doc["throttled_runs"].as_u64().ok_or("no throttled")?
        + doc["exhausted_runs"].as_u64().ok_or("no exhausted")?;
    assert_eq!(samples + given_up, 40);
    assert_eq!(latency["samples"].as_u64(), Some(samples));
    assert!(doc["lgwks_bot_version"].is_string() && doc["host"].is_string());
    assert_eq!(doc["seed"].as_u64(), Some(3));
    Ok(())
}

#[test]
fn the_human_report_carries_the_numbers_the_json_carries() -> TestResult {
    let scratch = scratch::Scratch::new("durable-cli-same")?;
    let dir = scratch.path().to_str().ok_or("the path is not UTF-8")?;
    let base = ["--runs", "12", "--seed", "9", "--work-dir", dir];
    let human = run(&base)?;
    let json = run(&[&base[..], &["--json"]].concat())?;
    assert!(human.status.success() && json.status.success());
    let text = String::from_utf8(human.stdout)?;
    let doc = lgwks_std::json::from_str::<Value>(&String::from_utf8(json.stdout)?)?;
    for key in [
        "samples",
        "failures_injected",
        "throttled_runs",
        "exhausted_runs",
    ] {
        let wanted = doc[key].as_u64().ok_or("a key is missing")?.to_string();
        assert_eq!(row(&text, key), Some(wanted.as_str()), "{key}");
    }
    assert!(row(&text, "latency_us").is_some_and(|line| line.contains("p99=")));
    assert!(row(&text, "peak_rss_bytes").is_some());
    assert_eq!(row(&text, "seed"), Some("9"));
    Ok(())
}

#[test]
fn recovery_after_a_real_kill_loses_no_run_and_reruns_no_step() -> TestResult {
    let out = run(&[
        "--runs",
        "20",
        "--recover",
        "--recover-rounds",
        "2",
        "--json",
    ])?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Progress counts rounds from one, one line per round, on stderr only.
    let progress = String::from_utf8(out.stderr.clone())?;
    let rounds: Vec<&str> = progress
        .lines()
        .filter(|line| line.starts_with("round "))
        .collect();
    assert_eq!(rounds.len(), 2, "{progress}");
    assert!(rounds[0].starts_with("round 1 of 2: "), "{progress}");
    assert!(rounds[1].starts_with("round 2 of 2: "), "{progress}");
    let doc = lgwks_std::json::from_str::<Value>(&String::from_utf8(out.stdout)?)?;
    let recovery = &doc["recovery"];
    assert_eq!(recovery["rounds"].as_u64(), Some(2));
    assert_eq!(recovery["duplicate_step_executions"].as_u64(), Some(0));
    assert!(
        recovery["kill_to_done_us"]["p50"]
            .as_u64()
            .ok_or("no p50")?
            > 0
    );
    assert!(recovery["resumed_runs"].as_u64().ok_or("no resumed")? > 0);
    assert!(recovery["steps_checked"].as_u64().ok_or("no steps")? > 0);
    Ok(())
}
