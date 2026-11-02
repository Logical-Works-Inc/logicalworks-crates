//! `--json` on every command: present and parseable, with the same facts.
//!
//! Each command runs once as a human and once as an agent: the JSON parses,
//! carries the documented keys, and agrees with the human verdict. The run
//! command's JSON prints before the app takes over stdout, so the first line
//! is the whole fact.

use std::error::Error;
use std::io::BufRead;
use std::net::SocketAddr;
use std::time::Duration;

use crate::lgwks_scratch::{
    Scratch, poll_ready, reference_app, run_lgwks, spawn_run, stop_run, test_port, unique_tag,
};
use crate::replay::refuse;

/// What this test returns.
type TestResult = Result<(), Box<dyn Error>>;

/// The JSON value on `stdout`, or the refusal to read it.
fn parse_stdout(stdout: &[u8], command: &str) -> Result<lgwks_std::json::Value, Box<dyn Error>> {
    lgwks_std::json::from_slice(stdout)
        .map_err(|error| format!("{command} --json is not JSON: {error}").into())
}

/// The boolean `ok` the verdict carries.
fn verdict_ok(value: &lgwks_std::json::Value, command: &str) -> Result<bool, Box<dyn Error>> {
    match value.get("ok").and_then(lgwks_std::json::Value::as_bool) {
        Some(ok) => Ok(ok),
        None => Err(format!("{command} --json carries no boolean ok").into()),
    }
}

/// New answers its two files as JSON.
#[test]
fn new_json_is_parseable() -> TestResult {
    let scratch = Scratch::fresh("json-new").map_err(|error| format!("no scratch: {error}"))?;
    let outcome = run_lgwks(scratch.dir(), &["new", "jsonapp", "--json"])?;
    if outcome.code != Some(0) {
        return refuse(format!("lgwks new --json refused: {}", outcome.stderr));
    }
    let value = parse_stdout(outcome.stdout.as_slice(), "lgwks new")?;
    if !verdict_ok(&value, "lgwks new")? {
        return refuse("lgwks new --json claims refusal");
    }
    match value.get("dir").and_then(lgwks_std::json::Value::as_str) {
        Some("jsonapp") => {}
        Some(other) => return refuse(format!("lgwks new --json names {other}")),
        None => return refuse("lgwks new --json carries no dir"),
    }
    match value
        .get("files")
        .and_then(lgwks_std::json::Value::as_array)
    {
        Some(files) if files.len() == 2 => {}
        Some(files) => return refuse(format!("lgwks new --json names {} files", files.len())),
        None => return refuse("lgwks new --json carries no files"),
    }
    Ok(())
}

/// Check answers the same verdict as text and as JSON.
#[test]
fn check_json_matches_the_human_verdict() -> TestResult {
    let scratch = Scratch::fresh("json-check").map_err(|error| format!("no scratch: {error}"))?;
    let name = String::from("jsoncheck");
    reference_app(scratch.dir(), name.as_str())?;
    let dir = scratch.dir().join(&name);
    let human = run_lgwks(&dir, &["check", "app.rs"])?;
    if human.code != Some(0) {
        return refuse(format!(
            "lgwks check refused the scaffold: {}",
            human.stderr
        ));
    }
    let machine = run_lgwks(&dir, &["check", "app.rs", "--json"])?;
    if machine.code != Some(0) {
        return refuse(format!(
            "lgwks check --json refused the scaffold: {}",
            machine.stderr
        ));
    }
    let value = parse_stdout(machine.stdout.as_slice(), "lgwks check")?;
    if !verdict_ok(&value, "lgwks check")? {
        return refuse("lgwks check --json disagrees with the human verdict");
    }
    Ok(())
}

/// A refused check carries location, kind, message and suggestion keys.
#[test]
fn check_json_refusal_shape() -> TestResult {
    let scratch = Scratch::fresh("json-refuse").map_err(|error| format!("no scratch: {error}"))?;
    let name = String::from("jsonrefuse");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let row = match reference
        .lines()
        .find(|line| line.trim_start().starts_with("route"))
    {
        Some(row) => row,
        None => return refuse("the scaffold holds no route line"),
    };
    let mutated = reference.replacen(
        row,
        "        frobnicate POST /drafts (body: Vec<u8>) -> Vec<u8>:",
        1,
    );
    let dir = scratch.dir().join(&name);
    std::fs::write(dir.join("app.rs"), mutated.as_str())
        .map_err(|error| format!("the mutated app could not be written: {error}"))?;
    let outcome = run_lgwks(&dir, &["check", "app.rs", "--json"])?;
    if outcome.code != Some(2) {
        return refuse("lgwks check --json admitted a bad word");
    }
    let value = parse_stdout(outcome.stdout.as_slice(), "lgwks check")?;
    if verdict_ok(&value, "lgwks check")? {
        return refuse("a refused check --json claims ok");
    }
    let refusal = match value
        .get("refusals")
        .and_then(lgwks_std::json::Value::as_array)
        .and_then(|refusals| refusals.first())
    {
        Some(refusal) => refusal,
        None => return refuse("a refused check --json carries no refusal"),
    };
    for key in ["location", "kind", "message", "suggestion"] {
        if refusal.get(key).is_none() {
            return refuse(format!("the refusal carries no {key}"));
        }
    }
    Ok(())
}

/// Expand answers its text as JSON.
#[test]
fn expand_json_is_parseable() -> TestResult {
    let scratch = Scratch::fresh("json-expand").map_err(|error| format!("no scratch: {error}"))?;
    let name = String::from("jsonexpand");
    reference_app(scratch.dir(), name.as_str())?;
    let dir = scratch.dir().join(&name);
    let outcome = run_lgwks(&dir, &["expand", "app.rs", "--json"])?;
    if outcome.code != Some(0) {
        return refuse(format!("lgwks expand --json refused: {}", outcome.stderr));
    }
    let value = parse_stdout(outcome.stdout.as_slice(), "lgwks expand")?;
    if !verdict_ok(&value, "lgwks expand")? {
        return refuse("lgwks expand --json claims refusal");
    }
    match value
        .get("expanded")
        .and_then(lgwks_std::json::Value::as_str)
    {
        Some(text) if text.contains("pub async fn drafts") => {}
        Some(_) => return refuse("lgwks expand --json holds no drafts flow"),
        None => return refuse("lgwks expand --json carries no expanded text"),
    }
    Ok(())
}

/// Run answers its timings as one JSON object on stdout, then serves.
#[test]
fn run_json_is_parseable_then_serves() -> TestResult {
    let scratch = Scratch::fresh("json-run").map_err(|error| format!("no scratch: {error}"))?;
    let name = unique_tag("jsonrun")?;
    reference_app(scratch.dir(), name.as_str())?;
    let dir = scratch.dir().join(&name);
    let port = test_port()?.saturating_add(1_000);
    let mut child = spawn_run(&dir, port, true)?;
    let outcome: TestResult = (|| {
        let mut line = String::new();
        match child.stdout.take() {
            Some(stdout) => {
                std::io::BufReader::new(stdout)
                    .read_line(&mut line)
                    .map_err(|error| format!("the timings could not be read: {error}"))?;
            }
            None => return refuse("lgwks run holds no stdout"),
        }
        if line.trim().is_empty() {
            return refuse("lgwks run --json printed no fact");
        }
        let value: lgwks_std::json::Value = lgwks_std::json::from_slice(line.as_bytes())
            .map_err(|error| format!("lgwks run --json is not JSON: {error}"))?;
        if !verdict_ok(&value, "lgwks run")? {
            return refuse("lgwks run --json claims refusal");
        }
        for key in ["cold_ms", "warm_ms", "start_ms", "port"] {
            if value
                .get(key)
                .and_then(lgwks_std::json::Value::as_u64)
                .is_none()
            {
                return refuse(format!("lgwks run --json carries no number {key}"));
            }
        }
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if !poll_ready(Duration::from_secs(300), Duration::from_millis(200), || {
            std::net::TcpStream::connect(addr).is_ok()
        }) {
            return refuse("the app never accepted");
        }
        Ok(())
    })();
    let stopped = stop_run(&mut child, port, Duration::from_secs(30));
    outcome?;
    stopped.map_err(Into::into)
}
