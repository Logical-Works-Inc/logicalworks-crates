//! The doc-test job's build profile (#1165).
//!
//! Source: the doc-test job set the run's wall when its build missed the
//! cache: 276 of 309 s on run 45931108189, 258 s of it compiling at the
//! workflow's opt-level 1, for doc tests that ran in 14 s.

use std::error::Error;
use std::path::Path;

/// The env block of `job` in `workflow`, between its header and `steps:`.
fn job_env<'a>(workflow: &'a str, job: &str) -> Option<&'a str> {
    let (_, after) = workflow.split_once(&format!("\n  {job}:\n"))?;
    let (head, _) = after.split_once("\n    steps:\n")?;
    head.split_once("\n    env:\n").map(|(_, env)| env)
}

/// Whether `env` sets the test profile's opt-level to 0.
fn at_opt_level_zero(env: &str) -> bool {
    env.lines()
        .any(|line| line.trim() == "CARGO_PROFILE_TEST_OPT_LEVEL: \"0\"")
}

/// The doc-test job builds at opt-level 0, set in the job's own env.
/// rust-cache hashes the `CARGO*` env when the job sets up, so a step's env
/// would keep main's opt-1 key, and every run would rebuild what it restored
/// (#272). Catches the setting dropped, and the setting moved to the step.
#[test]
fn the_doc_test_job_builds_at_opt_level_zero_under_its_own_key() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workflow = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))?;
    let env = job_env(&workflow, "doc_tests").ok_or("ci.yml's doc_tests job has no env")?;
    assert!(
        at_opt_level_zero(env),
        "doc_tests builds at the workflow's opt-level:\n{env}"
    );
    // The control: the same line under the step is not the job's.
    let moved = workflow
        .replacen("      CARGO_PROFILE_TEST_OPT_LEVEL: \"0\"\n    steps:\n", "    steps:\n", 1)
        .replacen(
            "      - name: Doc tests\n",
            "      - name: Doc tests\n        env:\n          CARGO_PROFILE_TEST_OPT_LEVEL: \"0\"\n",
            1,
        );
    assert_ne!(moved, workflow, "the control must change the workflow");
    let env = job_env(&moved, "doc_tests").ok_or("the control lost the job's env")?;
    assert!(
        !at_opt_level_zero(env),
        "a step's env read as the job's:\n{env}"
    );
    Ok(())
}
