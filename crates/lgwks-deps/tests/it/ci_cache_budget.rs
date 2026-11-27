//! The cache pruner's cutoff (ADR 0041, #1165).
//!
//! Source: main was red from run 45917420774 on. The pruner read each saving
//! workflow's latest *successful* main run, days old, so it pruned no main
//! entry, the store reached 10,335,771,053 bytes (run 45918166278), and
//! GitHub's own eviction dropped the doc-tests build. Every pull request then
//! compiled the doc tests cold and walled at 306 to 309 s.

use std::error::Error;
use std::path::Path;

/// Whether the pruner's query for a workflow's latest main run counts a
/// completed run that failed, and only a run that ran its jobs.
fn reads_completed_runs(budget: &str) -> bool {
    budget.contains("runs?branch=main&event=push&status=completed&per_page=20")
        && budget.contains(r#"select(.conclusion == "success" or .conclusion == "failure")"#)
}

/// A red main never turns the budget off: the pruner cuts off at each saving
/// workflow's latest completed main push run, failed included, because
/// rust-cache saves on failure and every job restores before it runs. A
/// cancelled or skipped run touched nothing, so it never sets the cutoff.
#[test]
fn the_pruner_cuts_off_at_the_latest_completed_main_run() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let budget = std::fs::read_to_string(root.join(".github/workflows/cache-budget.yml"))?;
    assert!(
        reads_completed_runs(&budget),
        "the pruner reads only successful main runs"
    );
    let success_only = budget.replacen(
        "status=completed&per_page=20",
        "status=success&per_page=1",
        1,
    );
    assert_ne!(success_only, budget, "the control must change the query");
    assert!(
        !reads_completed_runs(&success_only),
        "a success-only query read as completed"
    );
    let any_end = budget.replacen(
        r#"select(.conclusion == "success" or .conclusion == "failure")"#,
        "select(. != null)",
        1,
    );
    assert!(
        !reads_completed_runs(&any_end),
        "a cancelled run set the cutoff"
    );
    Ok(())
}
