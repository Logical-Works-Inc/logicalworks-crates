//! The `logical_ci` job runs on the free hosted Linux arm64 runner.
//!
//! Source: run 44143365086. A 3-slot hosted Mac ran A02's lane for 397 s and
//! killed the loom build at 123.7 s, where a 5-slot one passed in 121 s. The
//! verdict hung on which Mac GitHub handed out, and Mac minutes cost the most.
//! On Linux, lanes run uncontained (logical_ci#85), which bears on A08 alone;
//! this repository's policy gates A02. ADR 0029 records the move.

use std::path::{Path, PathBuf};

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The repository root this crate sits in.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Why `workflow` and `adr` do not put the gate on hosted Linux, one line per
/// way. Empty means they do.
fn runner_problems(workflow: &str, adr: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if !workflow.contains("    runs-on: ubuntu-24.04-arm\n")
        || workflow
            .lines()
            .any(|line| line.trim_start().starts_with("runs-on: macos"))
    {
        problems.push("logical_ci.yml runs the gate on a Mac".to_owned());
    }
    if workflow.contains("macos-arm64") {
        problems.push("logical_ci.yml runs the engine's macOS bytes".to_owned());
    }
    let Some(tag) = workflow
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("LCI_RELEASE:"))
        .map(str::trim)
    else {
        problems.push("logical_ci.yml sets no LCI_RELEASE".to_owned());
        return problems;
    };
    if !adr.contains(&format!("runs on `ubuntu-24.04-arm`: {tag} ships")) {
        problems.push(format!(
            "ADR 0029's runner line does not name Linux and {tag}"
        ));
    }
    problems
}

/// The committed job is on hosted Linux with the Linux engine, and each way
/// back to the Mac is named on its own.
#[test]
fn the_logical_ci_gate_runs_on_hosted_linux() -> TestResult {
    let workflow = std::fs::read_to_string(root().join(".github/workflows/logical_ci.yml"))?;
    let adr = std::fs::read_to_string(
        root().join("docs/adr/0029-the-logical-ci-verdict-runs-in-this-repository.md"),
    )?;
    assert_eq!(runner_problems(&workflow, &adr), Vec::<String>::new());

    let on_mac = workflow.replacen("runs-on: ubuntu-24.04-arm\n", "runs-on: macos-latest\n", 1);
    assert_ne!(
        on_mac, workflow,
        "the runner line the control moves must exist"
    );
    assert_eq!(
        runner_problems(&on_mac, &adr),
        vec!["logical_ci.yml runs the gate on a Mac".to_owned()],
        "a job moved back to a Mac runner is named"
    );

    let mac_bytes = workflow.replacen("logical_ci-linux-arm64", "logical_ci-macos-arm64", 1);
    assert_ne!(
        mac_bytes, workflow,
        "the engine path the control swaps must exist"
    );
    assert_eq!(
        runner_problems(&mac_bytes, &adr),
        vec!["logical_ci.yml runs the engine's macOS bytes".to_owned()],
        "a job that runs the macOS engine on Linux is named"
    );

    let stale = adr.replacen(
        "runs on `ubuntu-24.04-arm`: v",
        "runs on `ubuntu-24.04-arm`: v9",
        1,
    );
    assert_ne!(stale, adr, "the ADR line the control mutates must exist");
    assert_eq!(
        runner_problems(&workflow, &stale).len(),
        1,
        "an ADR runner line left on another release is named"
    );
    Ok(())
}
