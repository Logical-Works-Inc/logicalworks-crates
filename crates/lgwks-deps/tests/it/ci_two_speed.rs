//! The two speeds of `ci.yml` (ADR 0086): a pull request runs the Linux inner
//! loop, and the merge queue runs that plus the jobs a pull request skips.
//!
//! Source: PR 1146, run 45434287873. Every job passed, and the required row
//! failed the 300 s wall at 310 s: the macOS `bot-it-darwin` row ran 278 s and
//! ended last. Gemini's CI review for the Director, 2026-11-24: "Stop running
//! the 17-job cross-OS matrix on every PR push", run it on the queued batch.
//! Requirement proved: the jobs that skip a pull request are exactly the
//! verdict's QUEUE_ONLY list; a pull request passes with them skipped; the
//! queue fails on any of them skipped; and a failed or skipped Linux job fails
//! the row on every event.

use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// The committed workflow.
fn workflow() -> Result<String, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml");
    Ok(fs::read_to_string(path)?)
}

/// Each job id of `text` with its job-level `if:`, when it has one.
fn job_conditions(text: &str) -> Vec<(&str, Option<&str>)> {
    let mut found: Vec<(&str, Option<&str>)> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.is_empty() {
            in_jobs = line == "jobs:";
            continue;
        }
        if !in_jobs {
            continue;
        }
        if let Some(id) = line
            .strip_prefix("  ")
            .filter(|rest| !rest.starts_with([' ', '#']))
            .and_then(|rest| rest.strip_suffix(':'))
        {
            found.push((id, None));
        } else if let Some(condition) = line.strip_prefix("    if: ")
            && let Some(job) = found.last_mut()
        {
            job.1 = Some(condition);
        }
    }
    found
}

/// The QUEUE_ONLY list of the verdict step.
fn queue_only(text: &str) -> Result<Vec<&str>, Box<dyn Error>> {
    let line = text
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("QUEUE_ONLY: "))
        .ok_or("the verdict names no QUEUE_ONLY jobs")?;
    Ok(line.split_whitespace().collect())
}

/// The body of step `name`'s `run: |` block, its indent removed.
fn step_body(text: &str, name: &str) -> Result<String, Box<dyn Error>> {
    let wanted = format!("- name: {name}");
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim() != wanted)
        .skip_while(|line| line.trim() != "run: |")
        .skip(1);
    let first = lines.next().ok_or("the step has no run body")?;
    let pad = " ".repeat(first.len().saturating_sub(first.trim_start().len()));
    let mut body = vec![first.trim_start()];
    for line in lines {
        match line.strip_prefix(&pad) {
            Some(rest) => body.push(rest),
            None if line.trim().is_empty() => body.push(""),
            None => break,
        }
    }
    Ok(body.join("\n"))
}

/// A scratch directory whose `bin/jq` answers the two reads the verdict makes
/// of `needs`: its keys, and one job's result, from `results`.
fn scratch(case: &str, results: &[(&str, &str)]) -> Result<PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ci-two-speed-{case}"));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(root.join("bin"))?;
    let keys: Vec<&str> = results.iter().map(|&(job, _)| job).collect();
    let arms: String = results
        .iter()
        .map(|&(job, result)| format!("  {job}) echo {result} ;;\n"))
        .collect();
    let jq = format!(
        "#!/bin/sh\ncat >/dev/null\nif [ \"$2\" = 'keys[]' ]; then printf '%s\\n' {}; exit 0; fi\ncase \"$4\" in\n{arms}  *) echo null ;;\nesac\n",
        keys.join(" ")
    );
    let path = root.join("bin/jq");
    fs::write(&path, jq)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(root)
}

/// Runs the verdict step for `event` over `results`; true when it passes.
fn verdict(case: &str, event: &str, results: &[(&str, &str)]) -> Result<bool, Box<dyn Error>> {
    let text = workflow()?;
    let root = scratch(case, results)?;
    let queue = queue_only(&text)?.join(" ");
    let ran = Command::new("/bin/bash")
        .arg("-c")
        .arg(step_body(&text, "Every workspace test job passed")?)
        .current_dir(&root)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("NEEDS", "{}")
        .env("QUEUE_ONLY", queue)
        .env("GITHUB_EVENT_NAME", event)
        .output()?;
    fs::remove_dir_all(&root)?;
    Ok(ran.status.success())
}

#[test]
fn the_queue_runs_what_a_pull_request_skips() -> TestResult {
    let text = workflow()?;
    assert!(
        text.contains("\n  merge_group:\n"),
        "the merge queue runs ci.yml"
    );
    let queue = queue_only(&text)?;
    let skipping: Vec<&str> = job_conditions(&text)
        .into_iter()
        .filter(|&(_, condition)| {
            condition.is_some_and(|text| text.contains("github.event_name != 'pull_request'"))
        })
        .map(|(id, _)| id)
        .collect();
    let (mut sorted_queue, mut sorted_skipping) = (queue.clone(), skipping.clone());
    sorted_queue.sort_unstable();
    sorted_skipping.sort_unstable();
    assert_eq!(
        sorted_queue, sorted_skipping,
        "QUEUE_ONLY is exactly the jobs that skip a pull request"
    );
    for linux in [
        "gate",
        "clippy",
        "tests_linux",
        "workspace_build",
        "tests_nodefault",
    ] {
        assert!(
            !queue.contains(&linux),
            "{linux} runs on every pull request"
        );
    }
    let passed = [
        ("changes", "success"),
        ("tests_linux", "success"),
        ("tests_macos", "skipped"),
        ("gpui-windows", "skipped"),
    ];
    assert!(verdict("pr-pass", "pull_request", &passed)?);
    assert!(
        !verdict("queue-skipped", "merge_group", &passed)?,
        "the queue fails a skipped macOS row"
    );
    let all = [("tests_linux", "success"), ("tests_macos", "success")];
    assert!(verdict("queue-pass", "merge_group", &all)?);
    let linux_skipped = [("tests_linux", "skipped"), ("tests_macos", "skipped")];
    assert!(
        !verdict("pr-linux-skipped", "pull_request", &linux_skipped)?,
        "a pull request never skips a Linux suite"
    );
    let failed = [("tests_linux", "success"), ("tests_macos", "failure")];
    assert!(!verdict("queue-failed", "merge_group", &failed)?);
    assert!(
        !verdict("pr-failed", "pull_request", &failed)?,
        "a queue job that ran and failed fails a pull request too"
    );
    Ok(())
}
