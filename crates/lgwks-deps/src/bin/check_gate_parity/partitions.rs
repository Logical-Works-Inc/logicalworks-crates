//! Rule 8: a sliced row runs its slice.
//!
//! Source: #923 run 43232168371. The macOS T-row step ran without
//! `--partition`, so a group could not be split, and on a cold cache the g7
//! row's step took 237 s and the run's wall reached 312 s of its 300. A job
//! that gives a suite `partition: "slice:..."` rows must pass `--partition` in
//! every step that runs that suite; otherwise every slice reruns the whole set
//! and the split buys nothing.

//!
//! A sliced row also runs only its slice, so the rows of one shard must name
//! each of `slice:1/M` to `slice:M/M` once. A row dropped, a slice named twice
//! or two cuts of one shard leave tests that no job runs, and every job still
//! ends green. bot-it-g7 became two slices after a slow runner cancelled it at
//! its 5 minute cap (run 45399189913).

use std::collections::{BTreeMap, BTreeSet};

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
    /// Each sliced shard, by `archive` and `it_group`: every `(N, M)` its
    /// rows name, in file order.
    slices: BTreeMap<String, Vec<Slice>>,
    /// The job's steps, in file order.
    steps: Vec<StepText<'a>>,
}

/// The value of `key` in one inline matrix row, `- { key: VALUE, ... }`.
fn row_field<'a>(row: &'a str, key: &str) -> Option<&'a str> {
    let rest = row.split_once(&format!(" {key}:"))?.1.trim_start();
    rest.split([',', '}']).next().map(str::trim)
}

/// The suite of one inline matrix row, `- { suite: NAME, ... }`.
fn row_suite(row: &str) -> Option<&str> {
    row_field(row, "suite")
}

/// One row's slice: its `(N, M)`, or the text that is not `N/M`.
type Slice = Result<(u32, u32), String>;

/// The shard a sliced row belongs to and its slice, from
/// `archive: A, it_group: G, partition: "slice:N/M"`.
fn row_slice(row: &str) -> Option<(String, Slice)> {
    let text = row.split_once("partition: \"slice:")?.1.split('"').next()?;
    let shard = match (row_field(row, "archive"), row_field(row, "it_group")) {
        (Some(archive), Some(group)) => format!("{archive} {group}"),
        (Some(archive), None) => archive.to_owned(),
        (None, _) => row_suite(row)?.to_owned(),
    };
    let parsed = match text.split_once('/') {
        Some((part, of)) => match (part.trim().parse::<u32>(), of.trim().parse::<u32>()) {
            (Ok(part), Ok(of)) => Ok((part, of)),
            (Err(error), _) | (_, Err(error)) => Err(format!("{text:?} ({error})")),
        },
        None => Err(format!("{text:?} (no `/`)")),
    };
    Some((shard, parsed))
}

/// Why the slices of one shard do not run each of its tests once, or `None`.
fn slice_gap(workflow: &str, job: &str, shard: &str, slices: &[Slice]) -> Option<String> {
    if let Some(Err(text)) = slices.iter().find(|slice| slice.is_err()).cloned() {
        return Some(format!(
            "{workflow}: job {job} names {shard} slice {text}, which is not N/M (rule 9)"
        ));
    }
    let slices: Vec<(u32, u32)> = slices.iter().flatten().copied().collect();
    let cuts: BTreeSet<u32> = slices.iter().map(|&(_, of)| of).collect();
    let mut parts: Vec<u32> = slices.iter().map(|&(part, _)| part).collect();
    parts.sort_unstable();
    let cut: Vec<u32> = cuts.iter().copied().collect();
    match *cut.as_slice() {
        [of] if parts.iter().copied().eq(1..=of) => None,
        [of] => Some(format!(
            "{workflow}: job {job} runs {shard} as slices {parts:?} of {of}, not each of 1 to \
             {of} once, so some of its tests run in no job (rule 9)"
        )),
        _ => Some(format!(
            "{workflow}: job {job} cuts {shard} {cut:?} ways at once, so some of its tests run \
             in no job (rule 9)"
        )),
    }
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
                    slices: BTreeMap::new(),
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
            if let Some((shard, slice)) = row_slice(trimmed) {
                job.slices.entry(shard).or_default().push(slice);
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
                     --partition, so every slice reruns the whole set (rule 9)",
                    job.id, step.name
                ));
            }
        }
    }
    problems
}

/// The rule-9 slice violations of one workflow: every shard whose rows do not
/// name each of its slices once, by job and shard.
fn slice_problems(workflow: &str, text: &str) -> Vec<String> {
    jobs(text)
        .iter()
        .flat_map(|job| {
            job.slices
                .iter()
                .filter_map(|(shard, slices)| slice_gap(workflow, job.id, shard, slices))
        })
        .collect()
}

/// Every rule-9 violation of one workflow: steps that drop the slice, and
/// shards whose slices do not cover their tests once.
pub(crate) fn problems(workflow: &str, text: &str) -> Vec<String> {
    let mut found = partition_problems(workflow, text);
    found.extend(slice_problems(workflow, text));
    found
}

#[cfg(test)]
mod tests {
    use super::{partition_problems, slice_problems};

    /// The fat-finger: a slice that is not `N/M` is named as that, never
    /// read as a missing slice, and a shard whose rows cover it passes.
    #[test]
    fn partitions_a_slice_that_is_not_n_of_m_is_named() {
        let row = |slice: &str| {
            format!(
                "          - {{ suite: bot-full, archive: g7, partition: \"slice:{slice}\" }}\n"
            )
        };
        let text = |second: &str| {
            format!(
                "name: ci\njobs:\n  shard:\n    strategy:\n{}{}",
                row("1/2"),
                row(second)
            )
        };
        assert_eq!(slice_problems("ci.yml", &text("2/2")), Vec::<String>::new());
        for (second, named) in [("2/x", "\"2/x\" ("), ("two", "\"two\" (no `/`)")] {
            let problems = slice_problems("ci.yml", &text(second));
            assert_eq!(problems.len(), 1, "{second}: {problems:?}");
            assert!(
                problems
                    .iter()
                    .all(|problem| problem.contains(named) && problem.contains("not N/M")),
                "{second}: {problems:?}"
            );
        }
    }

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
