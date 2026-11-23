//! Rule 11: the doc tests stay off the one critical path (#1095).
//!
//! As a step of the evidence job they ran after the journey, about 200 s, so
//! the end-to-end row and the required row waited on them and the run walled
//! at 313 s with every test green (run 45086616792). Under the shared
//! `workspace` cache key another job's `dev` build owned the entry, so each run
//! compiled 249 crates again under the test profile, 3 m 19 s on an exact hit
//! (run 45056931513). So the job that runs them is needed by the required row
//! alone, and restores a cache key no other job uses. The cases are in
//! `tests/it/gate_parity_doc_path.rs`, which drives this binary.

#[cfg(test)]
#[path = "sim_doc_path.rs"]
mod sim_doc_path;

/// The step that runs the doc tests.
const DOC_STEP: &str = "- name: Doc tests";

/// The one job that may wait on them: the required row, which needs every job.
const VERDICT: &str = "build_verdict";

/// One job of the workflow, as the rule reads it.
struct Block<'a> {
    /// The job id.
    id: &'a str,
    /// The job ids its `needs:` names.
    needs: Vec<&'a str>,
    /// Every `cache-key:` value of its steps.
    keys: Vec<&'a str>,
    /// Whether one of its steps is the doc tests.
    doc: bool,
}

/// The job ids of an inline `needs:` value: one id or a `[a, b]` list, quoted
/// or not, with any trailing comment dropped.
fn inline_needs(value: &str) -> Vec<&str> {
    let value = match value.split_once('#') {
        Some((before, _)) => before,
        None => value,
    };
    value
        .trim()
        .trim_matches(['[', ']'])
        .split(',')
        .map(|id| id.trim().trim_matches(['"', '\'']))
        .filter(|id| !id.is_empty())
        .collect()
}

/// The jobs of `workflow`, in order: a job opens at a two-space `<id>:` line
/// under `jobs:`. `needs:` is read inline or as a block list.
fn blocks(workflow: &str) -> Vec<Block<'_>> {
    let mut found: Vec<Block<'_>> = Vec::new();
    let mut in_jobs = false;
    let mut listing = false;
    for line in workflow.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if listing {
            if let (Some(item), Some(job)) = (line.strip_prefix("      - "), found.last_mut()) {
                job.needs.extend(inline_needs(item));
                continue;
            }
            listing = false;
        }
        if let Some(id) = line
            .strip_prefix("  ")
            .and_then(|rest| rest.strip_suffix(':'))
            .filter(|id| !id.is_empty() && !id.contains([' ', '#']))
        {
            found.push(Block {
                id,
                needs: Vec::new(),
                keys: Vec::new(),
                doc: false,
            });
            continue;
        }
        let Some(job) = found.last_mut() else {
            continue;
        };
        let text = line.trim_start();
        if line.trim_end() == "    needs:" {
            listing = true;
        } else if let Some(needs) = line.strip_prefix("    needs: ") {
            job.needs = inline_needs(needs);
        } else if let Some(key) = text.strip_prefix("cache-key: ") {
            job.keys.push(key.trim());
        } else if text == DOC_STEP {
            job.doc = true;
        }
    }
    found
}

/// Every way `workflow` puts the doc tests back on the critical path.
pub(super) fn problems(workflow: &str) -> Vec<String> {
    let jobs = blocks(workflow);
    if !jobs.iter().any(|job| job.doc) {
        return vec!["ci.yml runs no `Doc tests` step".to_owned()];
    }
    let mut problems = Vec::new();
    for doc in jobs.iter().filter(|job| job.doc) {
        for job in &jobs {
            if job.id != VERDICT && job.needs.contains(&doc.id) {
                problems.push(format!(
                    "{} waits on {}, which runs the doc tests: only {VERDICT} may (#1095)",
                    job.id, doc.id
                ));
            }
        }
        if doc.keys.is_empty() {
            problems.push(format!("{} restores no dependency build (#1095)", doc.id));
        }
        let mut keys = doc.keys.clone();
        keys.sort_unstable();
        keys.dedup();
        for key in &keys {
            for other in jobs.iter().filter(|job| job.id != doc.id) {
                if other.keys.contains(key) {
                    problems.push(format!(
                        "{}'s cache key {key} is also {}'s: the first saver owns the entry (#1095)",
                        doc.id, other.id
                    ));
                }
            }
        }
    }
    problems
}
