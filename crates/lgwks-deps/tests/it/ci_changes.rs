//! The `Classify the change` step of `ci.yml`, run as CI runs it.
//!
//! Source: PR 1142, run 45417609208. GitHub answered the pull request's file
//! listing with HTTP 500 ("this diff is temporarily unavailable due to heavy
//! server load"). Under `set -euo pipefail` the step exited 1, so the required
//! `Build and workspace tests` row failed with every test unrun. Requirement
//! proved (INV-CI-DOCS-ONLY-1): a listing that fails is code, so the suites
//! run, and a listing of only reader-facing pages is not.

use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// The body of the step's `run: |` block, its indent removed.
fn step_body() -> Result<String, Box<dyn Error>> {
    let workflow = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml");
    let text = fs::read_to_string(workflow)?;
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim() != "- name: Classify the change")
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

/// Runs the step with a `gh` that prints `listing` and exits `exit`, and
/// returns the step's exit code and what it wrote to `GITHUB_OUTPUT`.
fn classify(case: &str, listing: &str, exit: i32) -> Result<(Option<i32>, String), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ci-changes-{case}"));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    let gh = root.join("gh");
    fs::write(
        &gh,
        format!("#!/bin/sh\nprintf '%s' '{listing}'\nexit {exit}\n"),
    )?;
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755))?;
    let output = root.join("output");
    fs::write(&output, "")?;
    let path = format!("{}:/usr/bin:/bin", root.display());
    let status = Command::new("/bin/bash")
        .arg("-c")
        .arg(step_body()?)
        .env_clear()
        .env("PATH", path)
        .env("PR", "1142")
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("GITHUB_OUTPUT", &output)
        .status()?;
    let written = fs::read_to_string(&output)?;
    fs::remove_dir_all(&root)?;
    Ok((status.code(), written))
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
