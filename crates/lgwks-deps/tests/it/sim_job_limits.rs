//! The job limit of every `ci.yml` job, under a fat-fingered editor.
//!
//! Source: run 45492643765, main's first push after kache landed. Every
//! compiling job hit "The job has exceeded the maximum execution time of
//! 5m0s" still compiling the workspace cold through kache (lgwks_bot alone
//! took 3 m 53 s). A cancelled job drains no uploads ("uploads still queued
//! are lost") and saves no build. So the next pull request, run 45498050734,
//! read 30 of 403 units from the store and its required row ended at 323 s.
//! Requirement proved (ADR 0086 decision 2): a push to main outlives its cold
//! compile and writes the store, and a pull request keeps the 5-minute gate.
//!
//! Each seed draws a set of jobs and gives each a wrong limit: 5 everywhere,
//! 15 everywhere, the two arms swapped, keyed on the wrong event, padded with
//! spaces, or removed. The reference model is the drawn set itself: exactly
//! those jobs, and no other, are refused.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::ci_compile_cache::jobs;
use crate::replay::{Failure, refuse, replay_sweep};
use crate::sim::{Rng, Trace};

/// The job limit every job of `ci.yml` carries: 5 minutes on a pull request
/// and in the merge queue, 15 on a push to `main`.
const JOB_LIMIT: &str = "timeout-minutes: ${{ github.event_name == 'push' && 15 || 5 }}";

/// What a careless edit leaves in place of [`JOB_LIMIT`]. Each is wrong.
const WRONG: [&str; 6] = [
    "timeout-minutes: 5",
    "timeout-minutes: 15",
    "timeout-minutes: ${{ github.event_name == 'push' && 5 || 15 }}",
    "timeout-minutes: ${{ github.event_name == 'pull_request' && 15 || 5 }}",
    "timeout-minutes: ${{ github.event_name == 'push' && 15 || 5 }}  ",
    "",
];

const SEEDS: u64 = 64;

/// The step limits of `workflow` that do not leave the job time to save its
/// cache: each must be `${{ github.event_name == 'push' && P || Q }}` with Q
/// under 5 and P under 15. Run 45532085813: a fixed 4-minute Windows step cut
/// main's cold compile at test 94 of 119, though its job had 15 minutes.
fn bad_step_limits(workflow: &str) -> Vec<String> {
    let arms = |line: &str| -> Option<(u32, u32)> {
        let rest =
            line.strip_prefix("        timeout-minutes: ${{ github.event_name == 'push' && ")?;
        let (push, rest) = rest.split_once(" || ")?;
        Some((push.parse().ok()?, rest.strip_suffix(" }}")?.parse().ok()?))
    };
    workflow
        .lines()
        .filter(|line| line.starts_with("        timeout-minutes:"))
        .filter(|line| !arms(line).is_some_and(|(push, other)| push < 15 && other < 5))
        .map(str::to_owned)
        .collect()
}

/// Each job of `workflow` whose limit is not [`JOB_LIMIT`], by id.
fn short_limits(workflow: &str) -> BTreeSet<&str> {
    jobs(workflow)
        .into_iter()
        .filter(|job| !job.1.contains(&format!("\n    {JOB_LIMIT}\n")))
        .map(|(id, _)| id)
        .collect()
}

/// `workflow` with job `id`'s limit line replaced by `wrong`.
fn with_limit(workflow: &str, id: &str, wrong: &str) -> Option<String> {
    let start = workflow.find(&format!("\n  {id}:\n"))?;
    let line = format!("\n    {JOB_LIMIT}\n");
    let at = start.checked_add(workflow.get(start..)?.find(&line)?)?;
    let end = at.checked_add(line.len())?;
    let replacement = if wrong.is_empty() {
        "\n".to_owned()
    } else {
        format!("\n    {wrong}\n")
    };
    let mut edited = workflow.to_owned();
    edited.replace_range(at..end, &replacement);
    Some(edited)
}

#[test]
fn sim_every_wrong_job_limit_is_refused_by_name() -> Result<(), Failure> {
    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml"),
    )?;
    let held = short_limits(&workflow);
    if !held.is_empty() {
        return refuse(format!("ci.yml holds jobs to another limit: {held:?}"));
    }
    let steps = bad_step_limits(&workflow);
    if !steps.is_empty() {
        return refuse(format!("step limits not keyed on the event: {steps:?}"));
    }
    for wrong in [
        "        timeout-minutes: 4",
        "        timeout-minutes: ${{ github.event_name == 'push' && 15 || 4 }}",
        "        timeout-minutes: ${{ github.event_name == 'push' && 14 || 5 }}",
    ] {
        if bad_step_limits(wrong).len() != 1 {
            return refuse(format!("a wrong step limit passes: {wrong}"));
        }
    }
    let ids: Vec<&str> = jobs(&workflow).into_iter().map(|(id, _)| id).collect();
    if ids.len() < 20 {
        return refuse(format!("ci.yml lists only {} jobs", ids.len()));
    }
    replay_sweep("job-limits", SEEDS, |rng: &mut Rng, trace: &mut Trace| {
        let mut edited = workflow.clone();
        let mut drawn = BTreeSet::new();
        for _ in 0..rng.between(1, 6) {
            let Some(&id) = rng.pick(&ids) else {
                return refuse("no job to draw");
            };
            let Some(&wrong) = rng.pick(&WRONG) else {
                return refuse("no wrong limit to draw");
            };
            if !drawn.insert(id) {
                continue;
            }
            let Some(next) = with_limit(&edited, id, wrong) else {
                return refuse(format!("job {id} has no limit line to edit"));
            };
            edited = next;
            trace.record_number("wrong", u64::try_from(wrong.len())?);
        }
        let refused = short_limits(&edited);
        if refused != drawn {
            return refuse(format!("edited {drawn:?}, refused {refused:?}"));
        }
        trace.record_number("drawn", u64::try_from(drawn.len())?);
        Ok(())
    })
}
