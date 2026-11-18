//! The `logical_ci` job runs where the pinned engine contains its lanes.
//!
//! Source: run 44148839889. On `ubuntu-24.04-arm` logical_ci v0.4.3 refused `ci` before any lane
//! ran: "open containment: ... the run is refused until per-run key isolation lands" (exit 2,
//! UNMEASURED), so every pull request of the stack was red. Linux gains containment with
//! logical_ci#85; until then the job runs on the hosted Mac, where `sandbox-exec` contains lanes.

use std::path::{Path, PathBuf};

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The repository root this crate sits in.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Why `workflow` does not run the engine where it contains lanes, one line per way.
fn runner_problems(workflow: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let runner = workflow
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("runs-on:"))
        .map(str::trim);
    if runner != Some("macos-latest") {
        problems.push(format!(
            "logical_ci.yml runs the gate on {runner:?}, where the engine refuses `ci` (logical_ci#85)"
        ));
    }
    if workflow.contains("logical_ci-linux-arm64") {
        problems.push("logical_ci.yml runs the engine's Linux bytes".to_owned());
    }
    problems
}

/// The committed job runs on the Mac with the macOS engine, and each way to Linux is named.
#[test]
fn the_logical_ci_gate_runs_where_the_engine_contains_lanes() -> TestResult {
    let workflow = std::fs::read_to_string(root().join(".github/workflows/logical_ci.yml"))?;
    assert_eq!(runner_problems(&workflow), Vec::<String>::new());

    let on_linux = workflow.replacen("runs-on: macos-latest\n", "runs-on: ubuntu-24.04-arm\n", 1);
    assert_ne!(
        on_linux, workflow,
        "the runner line the control moves must exist"
    );
    assert_eq!(
        runner_problems(&on_linux).len(),
        1,
        "a job moved to Linux before containment is named"
    );

    let linux_bytes = workflow.replacen("logical_ci-macos-arm64", "logical_ci-linux-arm64", 1);
    assert_ne!(
        linux_bytes, workflow,
        "the engine path the control swaps must exist"
    );
    assert_eq!(
        runner_problems(&linux_bytes),
        vec!["logical_ci.yml runs the engine's Linux bytes".to_owned()],
        "a job that runs the Linux engine is named"
    );
    Ok(())
}
