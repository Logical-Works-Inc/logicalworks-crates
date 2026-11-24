//! Steps of `ci.yml`, each run as CI runs it: its own body under bash, with
//! fake tools on `PATH` in a scratch directory.
//!
//! Source: PR 1142, run 45417609208. GitHub answered the pull request's file
//! listing with HTTP 500 ("this diff is temporarily unavailable due to heavy
//! server load"). Under `set -euo pipefail` the step exited 1, so the required
//! `Build and workspace tests` row failed with every test unrun. Requirement
//! proved (INV-CI-DOCS-ONLY-1): a listing that fails is code, so the suites
//! run, and a listing of only reader-facing pages is not.
//!
//! Source: PR 1146, run 45418938734. The wait step saw the bot-rest listing in
//! the REST API at 14:55:09.5, and `download-artifact` read the artifact
//! service 0.3 s later and found 33 artifacts without it, so the simulation
//! lane failed on a missing file. Requirement proved: a listing the download
//! missed is fetched by name, and one that never comes fails by name.

use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn Error>>;

/// The body of step `name`'s `run: |` block, its indent removed.
fn step_body(name: &str) -> Result<String, Box<dyn Error>> {
    let workflow = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml");
    let text = fs::read_to_string(workflow)?;
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

/// A fresh scratch directory for one case.
fn scratch(case: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ci-step-{case}"));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(root.join("bin"))?;
    Ok(root)
}

/// Writes an executable `/bin/sh` script `name` into `root/bin`.
fn tool(root: &Path, name: &str, script: &str) -> TestResult {
    let path = root.join("bin").join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// Runs step `name` in `root` with `root/bin` first on `PATH`.
fn run_step(root: &Path, name: &str, env: &[(&str, &str)]) -> Result<Output, Box<dyn Error>> {
    let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
    let mut command = Command::new("/bin/bash");
    command
        .arg("-c")
        .arg(step_body(name)?)
        .current_dir(root)
        .env_clear()
        .env("PATH", path)
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("GITHUB_API_URL", "https://api.invalid")
        .env("GITHUB_RUN_ID", "7");
    for &(key, value) in env {
        command.env(key, value);
    }
    Ok(command.output()?)
}

/// Runs the classify step with a `gh` that prints `listing` and exits `exit`,
/// and returns the step's exit code and what it wrote to `GITHUB_OUTPUT`.
fn classify(case: &str, listing: &str, exit: i32) -> Result<(Option<i32>, String), Box<dyn Error>> {
    let root = scratch(case)?;
    tool(
        &root,
        "gh",
        &format!("printf '%s' '{listing}'\nexit {exit}\n"),
    )?;
    let output = root.join("output");
    fs::write(&output, "")?;
    let shown = output.display().to_string();
    let ran = run_step(
        &root,
        "Classify the change",
        &[("PR", "1142"), ("GITHUB_OUTPUT", &shown)],
    )?;
    let written = fs::read_to_string(&output)?;
    fs::remove_dir_all(&root)?;
    Ok((ran.status.code(), written))
}

#[test]
fn a_failed_listing_is_code_and_a_docs_listing_is_not() -> TestResult {
    let cases = [
        ("server-error", "", 1, "code=true\n"),
        (
            "docs-only",
            "docs/guide.md\nllms.txt\nGOVERNANCE.md",
            0,
            "code=false\n",
        ),
        (
            "rust",
            "docs/guide.md\ncrates/lgwks-deps/src/lib.rs",
            0,
            "code=true\n",
        ),
        ("generated", "docs/catalog.md", 0, "code=true\n"),
        ("empty", "", 0, "code=true\n"),
    ];
    for (case, listing, exit, wanted) in cases {
        let (code, written) = classify(case, listing, exit)?;
        assert_eq!(code, Some(0), "{case}: the step must not fail the run");
        assert_eq!(written, wanted, "{case}");
    }
    Ok(())
}

/// Each job of `text` with its body, a key two spaces under `jobs:`.
fn jobs(text: &str) -> Vec<(&str, String)> {
    let mut found: Vec<(&str, String)> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.is_empty() {
            in_jobs = line == "jobs:";
            continue;
        }
        let key = line
            .strip_prefix("  ")
            .filter(|rest| !rest.starts_with([' ', '#']));
        match key.and_then(|rest| rest.strip_suffix(':')) {
            Some(id) if in_jobs => found.push((id, String::new())),
            _ => {
                if let Some(job) = found.last_mut() {
                    job.1.push_str(line);
                    job.1.push('\n');
                }
            }
        }
    }
    found
}

/// Source: PR 1146, run 45426215266. The bench workspace is its own Cargo
/// root, and rust-cache saved only the root `target`, so every run compiled
/// it cold twice: 64 s for the tests and 127 s for the LTO release build on
/// run 45419917546. On a slower runner it was still compiling at the 5
/// minute cap. Requirement proved: a job that builds a nested workspace
/// passes that workspace to the cache, and the action hands it to
/// rust-cache.
#[test]
fn a_nested_workspace_job_caches_its_own_target() -> TestResult {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let action = fs::read_to_string(repo.join(".github/actions/hosted-rust/action.yml"))?;
    assert!(
        action.contains("workspaces: ${{ inputs.workspace }} -> target"),
        "hosted-rust hands its workspace to rust-cache"
    );
    let workflow = fs::read_to_string(repo.join(".github/workflows/ci.yml"))?;
    let mut nested = Vec::new();
    for (id, body) in jobs(&workflow) {
        let pieces: Vec<&str> = body.split("--manifest-path ").collect();
        for pair in pieces.windows(2) {
            let &[before, after] = pair else { continue };
            let Some(dir) = after
                .split_whitespace()
                .next()
                .and_then(|path| path.strip_suffix("/Cargo.toml"))
            else {
                continue;
            };
            let manifest = fs::read_to_string(repo.join(dir).join("Cargo.toml"))?;
            if !manifest.lines().any(|line| line == "[workspace]") {
                continue;
            }
            // The command the flag belongs to: from its `cargo` to the next `&&` or step.
            let Some((_, head)) = before.rsplit_once("cargo ") else {
                continue;
            };
            let tail = match after.split(" && ").next() {
                Some(command) => match command.split("\n      - ").next() {
                    Some(step) => step,
                    None => command,
                },
                None => after,
            };
            if head.starts_with("fmt")
                || head.contains("--target-dir")
                || tail.contains("--target-dir")
            {
                continue;
            }
            nested.push(id);
            assert!(
                body.contains(&format!("workspace: {dir}\n")),
                "job {id} builds {dir} but caches only the root target"
            );
        }
    }
    assert!(
        nested.contains(&"timing_async"),
        "the bench job builds a nested workspace: {nested:?}"
    );
    Ok(())
}

/// The fetch step, given a download that left out bot-rest. `serves` says
/// whether the REST API answers for it.
fn fetch_missing(case: &str, serves: bool) -> Result<(Output, bool), Box<dyn Error>> {
    let root = scratch(case)?;
    let listings = root.join("nextest-listing");
    let suites = [
        "workspace",
        "bot-it-scale",
        "bot-it-surface",
        "bot-it-refusals",
        "bot-it-g1",
        "bot-it-g2",
        "bot-it-g3",
        "bot-it-g4",
        "bot-it-g5",
        "bot-it-g6",
        "bot-it-g7",
        "bot-it-g8",
    ];
    for suite in suites {
        let dir = listings.join(format!("nextest-listing-{suite}-Linux"));
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("listing.json"), "{}")?;
    }
    let curl = if serves {
        "out=''; url=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -o) out=\"$2\"; shift ;; https*) url=\"$1\" ;; esac\n  shift\ndone\n\
         case \"$url\" in\n  *'artifacts?name=nextest-listing-bot-rest-Linux') printf '{\"total_count\": 1, \"artifacts\": [{\"id\": 42, \"name\": \"x\"}]}' ;;\n  \
         */artifacts/42/zip) printf 'zip' > \"$out\" ;;\n  *) exit 22 ;;\nesac\n"
    } else {
        "exit 22\n"
    };
    tool(&root, "curl", curl)?;
    tool(
        &root,
        "unzip",
        "dir=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -d) dir=\"$2\"; shift ;; esac\n  shift\ndone\n\
         mkdir -p \"$dir\" && printf '{\"rust-suites\": {}}' > \"$dir/listing.json\"\n",
    )?;
    let ran = run_step(
        &root,
        "Download any listing the first download missed",
        &[("TOKEN", "t"), ("LISTING_RETRY_SECS", "0")],
    )?;
    let fetched = listings
        .join("nextest-listing-bot-rest-Linux/listing.json")
        .exists();
    fs::remove_dir_all(&root)?;
    Ok((ran, fetched))
}

#[test]
fn a_listing_the_download_missed_is_fetched_by_name() -> TestResult {
    let (ran, fetched) = fetch_missing("fetch-served", true)?;
    let said = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert_eq!(ran.status.code(), Some(0), "{said}");
    assert!(fetched, "the bot-rest listing is fetched: {said}");
    let (ran, fetched) = fetch_missing("fetch-never", false)?;
    let said = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert_eq!(
        ran.status.code(),
        Some(1),
        "a listing that never comes fails: {said}"
    );
    assert!(!fetched);
    assert!(
        said.contains("bot-rest"),
        "the missing suite is named: {said}"
    );
    Ok(())
}
