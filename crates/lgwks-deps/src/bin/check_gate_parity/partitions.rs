//! Rule 8: a sliced row runs its slice.
//!
//! Source: #923 run 43232168371. The macOS T-row step ran without
//! `--partition`, so a group could not be split, and on a cold cache the g7
//! row's step took 237 s and the run's wall reached 312 s of its 300. A job
//! that gives a suite `partition: "slice:..."` rows must pass `--partition` in
//! every step that runs that suite; otherwise every slice reruns the whole set
//! and the split buys nothing.

use std::collections::BTreeSet;

/// One step of a job: its name, its `if:` condition and every line of it.
struct StepText<'a> {
    /// The step's `name:`, or its first line when it has none.
    name: &'a str,
    /// The text after `if:`, empty when the step always runs.
    condition: &'a str,
    /// Every line of the step, trimmed.
    lines: Vec<&'a str>,
}

/// The suites a job's matrix slices, and the job's steps.
struct JobText<'a> {
    /// The job's key under `jobs:`.
    id: &'a str,
    /// Each suite one of the job's matrix rows gives a `slice:` partition.
    sliced: BTreeSet<&'a str>,
    /// The job's steps, in file order.
    steps: Vec<StepText<'a>>,
}

/// The suite of one inline matrix row, `- { suite: NAME, ... }`.
fn row_suite(row: &str) -> Option<&str> {
    let rest = row.split_once("suite:")?.1.trim_start();
    rest.split([',', '}']).next().map(str::trim)
}

/// Every job of `text`, in file order: a key two spaces under `jobs:`.
fn jobs(text: &str) -> Vec<JobText<'_>> {
    let mut found: Vec<JobText<'_>> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len().saturating_sub(trimmed.len());
        if indent == 0 {
            in_jobs = trimmed == "jobs:";
            continue;
        }
        if !in_jobs {
            continue;
        }
        if indent == 2 {
            if let Some(id) = trimmed.strip_suffix(':') {
                found.push(JobText {
                    id,
                    sliced: BTreeSet::new(),
                    steps: Vec::new(),
                });
            }
            continue;
        }
        let Some(job) = found.last_mut() else {
            continue;
        };
        if trimmed.starts_with("- {") && trimmed.contains("partition: \"slice:") {
            if let Some(suite) = row_suite(trimmed) {
                job.sliced.insert(suite);
            }
        } else if let Some(name) = trimmed.strip_prefix("- name:") {
            job.steps.push(StepText {
                name: name.trim(),
                condition: "",
                lines: vec![trimmed],
            });
        } else if trimmed.starts_with("- ") && trimmed != "- {" {
            // An unnamed step (`- uses:`, `- run:`) ends the named one before it.
            job.steps.push(StepText {
                name: trimmed,
                condition: "",
                lines: vec![trimmed],
            });
        } else if let Some(step) = job.steps.last_mut() {
            if let Some(condition) = trimmed.strip_prefix("if:") {
                step.condition = condition.trim();
            }
            step.lines.push(trimmed);
        }
    }
    found
}

/// The rule-8 violations of one workflow: every step that runs a sliced suite
/// and passes no `--partition`, by job and step name.
pub(crate) fn partition_problems(workflow: &str, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for job in jobs(text) {
        for suite in &job.sliced {
            let wanted = format!("matrix.suite == '{suite}'");
            for step in &job.steps {
                if !step.condition.contains(&wanted) {
                    continue;
                }
                if step.lines.iter().any(|line| line.contains("--partition")) {
                    continue;
                }
                problems.push(format!(
                    "{workflow}: job {} slices suite {suite}, but step {:?} passes no \
                     --partition, so every slice reruns the whole set (rule 8)",
                    job.id, step.name
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::partition_problems;

    /// A job of two rows of `suite`, sliced or not, whose step runs `run`.
    fn workflow(partition: &str, condition: &str, run: &str) -> String {
        format!(
            "name: ci\njobs:\n  shard:\n    runs-on: macos-15\n    strategy:\n      matrix:\n        \
             include:\n          - {{ suite: bot-it, it_group: g7, partition: \"{partition}\" }}\n    \
             steps:\n      - uses: actions/checkout@v7\n      - name: Run the rows\n        \
             if: {condition}\n        run: {run}\n"
        )
    }

    /// The g7 shape of run 43232168371: a sliced row whose step drops the
    /// slice is refused, and naming the slice clears it.
    #[test]
    fn a_sliced_row_whose_step_runs_every_test_is_refused() {
        let dropped = workflow("slice:1/2", "matrix.suite == 'bot-it'", "cargo nextest run");
        let problems = partition_problems("ci.yml", &dropped);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems
                .iter()
                .all(|problem| problem.contains("Run the rows")),
            "{problems:?}"
        );
        let passed = workflow(
            "slice:1/2",
            "matrix.suite == 'bot-it'",
            "cargo nextest run ${PARTITION:+--partition $PARTITION}",
        );
        assert_eq!(partition_problems("ci.yml", &passed), Vec::<String>::new());
    }

    /// An unsliced row, and a step for another suite or one that excludes the
    /// suite, carry no obligation.
    #[test]
    fn an_unsliced_row_or_another_suites_step_owes_nothing() {
        for (partition, condition) in [
            ("", "matrix.suite == 'bot-it'"),
            ("slice:1/2", "matrix.suite == 'workspace'"),
            ("slice:1/2", "matrix.suite != 'bot-it'"),
        ] {
            let text = workflow(partition, condition, "cargo nextest run");
            assert_eq!(
                partition_problems("ci.yml", &text),
                Vec::<String>::new(),
                "{partition:?} under {condition:?}"
            );
        }
    }
}
