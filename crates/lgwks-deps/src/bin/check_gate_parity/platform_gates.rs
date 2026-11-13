//! The platform gates (ADR 0042): every suite runs on Linux, the cheapest
//! hosted runner, and macOS runs only what needs Darwin, so one run's macOS
//! jobs leave room for another's.
//!
//! Source: GitHub runs at most 50 hosted macOS jobs at once on this plan. Each
//! run had 27, so three overlapping runs queued macOS jobs up to 127 s and #943
//! failed its 300 s wall with every job green (run 43345936756). Ten of those
//! jobs ran saturation tiers no receipt names, and ten ran every Darwin-named
//! test where the Windows job runs only the row-named ones.

use std::error::Error;
use std::path::{Path, PathBuf};

use crate::WORKFLOW_REL;

type TestResult = Result<(), Box<dyn Error>>;

/// The macOS jobs one run may start: half the 50 GitHub runs at once, so two
/// runs never queue on macOS.
const MACOS_JOBS_PER_RUN: usize = 25;

/// The Darwin receipt's map and the features its rows build.
const DARWIN_RECEIPT: &str = "--map docs/acceptance/t-rows-darwin.toml --features rt,time,sync,macros,io,process,script,ephemeral,ecs,wire --platform darwin-arm64";

/// The filter of every Darwin `it` row: the row-named tests alone.
const DARWIN_ROW_FILTER: &str = "it_filter: \"binary(it) & test(/_t[0-9]{2}$/)\"";

/// The Linux saturation row's filter: every tier.
const EVERY_TIER: &str = "sat_filter: \"binary(sim_review_path) & test(/^saturation_r32_tier_/)\"";

/// The repository this crate sits in.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The leading spaces of `line`.
fn indent(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

/// The items of a `key: [a, b]` line, or `None` when it is not a list.
fn list_items(line: &str) -> Option<Vec<&str>> {
    let (_, value) = line.trim().split_once(": [")?;
    let inner = value.strip_suffix(']')?;
    Some(inner.split(',').map(str::trim).collect())
}

/// How many macOS jobs one job of the workflow starts: every matrix row of a
/// job on `macos-15`, and each `macos-15` row of a job on `matrix.os`.
fn job_macos(job: &[&str]) -> usize {
    let runs_on = job
        .iter()
        .find_map(|line| line.trim_start().strip_prefix("runs-on: "));
    let Some(runs_on) = runs_on else {
        return 0;
    };
    let rows: Vec<&str> = job
        .iter()
        .copied()
        .filter(|line| line.trim_start().starts_with("- {"))
        .collect();
    let mut lists: Vec<Vec<&str>> = Vec::new();
    let mut in_strategy = false;
    for &line in job {
        if line.trim() == "strategy:" {
            in_strategy = true;
        } else if in_strategy && indent(line) <= 4 && !line.trim().is_empty() {
            in_strategy = false;
        } else if in_strategy && let Some(items) = list_items(line) {
            lists.push(items);
        }
    }
    if runs_on.contains("macos-15") {
        if !rows.is_empty() {
            return rows.len();
        }
        return lists.iter().map(Vec::len).product();
    }
    if runs_on.contains("matrix.os") {
        let in_rows = rows
            .iter()
            .filter(|row| row.contains("os: macos-15"))
            .count();
        let in_lists: usize = lists
            .iter()
            .map(|items| items.iter().filter(|item| **item == "macos-15").count())
            .sum();
        return in_rows.saturating_add(in_lists);
    }
    0
}

/// The macOS jobs one run of `workflow` starts.
fn macos_jobs(workflow: &str) -> usize {
    let mut total = 0_usize;
    let mut job: Vec<&str> = Vec::new();
    let mut in_jobs = false;
    for line in workflow.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        let starts_job = indent(line) == 2 && line.trim_end().ends_with(':');
        if starts_job {
            total = total.saturating_add(job_macos(&job));
            job.clear();
        }
        job.push(line);
    }
    total.saturating_add(job_macos(&job))
}

/// Each row of an acceptance map: its id and the tests it names, in order.
fn map_rows(map: &str) -> Vec<(&str, Vec<&str>)> {
    let mut rows: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in map.lines() {
        if let Some(id) = line.strip_prefix("id = ") {
            rows.push((id.trim_matches('"'), Vec::new()));
        } else if let Some(name) = line
            .strip_prefix("  \"")
            .and_then(|rest| rest.strip_suffix("\","))
            && let Some(row) = rows.last_mut()
        {
            row.1.push(name);
        }
    }
    rows
}

/// Whether `name` carries a row id, `_tNN`.
fn row_named(name: &str) -> bool {
    name.rsplit_once("_t").is_some_and(|(_, digits)| {
        digits.len() == 2 && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// The gap sentence each Darwin row holds, by id.
fn gaps(map: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut id = "";
    for line in map.lines() {
        if let Some(found_id) = line.strip_prefix("id = ") {
            id = found_id.trim_matches('"');
        } else if let Some(gap) = line.strip_prefix("gap = ") {
            found.push((id, gap));
        }
    }
    found
}

/// Every way the committed files can put a suite back on macOS, by name.
fn platform_problems(ci: &str, full: &str, darwin: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let macos = macos_jobs(ci);
    if macos > MACOS_JOBS_PER_RUN {
        problems.push(format!(
            "ci.yml starts {macos} macOS jobs a run, past {MACOS_JOBS_PER_RUN}, half the 50 GitHub runs at once (ADR 0042)"
        ));
    }
    let darwin_rows = map_rows(darwin);
    let darwin_gaps = gaps(darwin);
    for (id, tests) in map_rows(full) {
        let wanted: Vec<&str> = tests
            .iter()
            .copied()
            .filter(|name| row_named(name))
            .collect();
        let named = darwin_rows
            .iter()
            .find(|row| row.0 == id)
            .map(|row| row.1.clone());
        if named.as_ref() != Some(&wanted) {
            problems.push(format!(
                "t-rows-darwin.toml's {id} names {named:?}, not the row's `_tNN` tests {wanted:?}"
            ));
        }
        let count = format!("covers {} of the {} tests", wanted.len(), tests.len());
        if !darwin_gaps
            .iter()
            .any(|&(gap_id, gap)| gap_id == id && gap.contains(&count))
        {
            problems.push(format!(
                "t-rows-darwin.toml's {id} gap does not say it {count}"
            ));
        }
    }
    let full_ids: Vec<&str> = map_rows(full).iter().map(|row| row.0).collect();
    for &(id, _) in &darwin_rows {
        if !full_ids.contains(&id) {
            problems.push(format!(
                "t-rows-darwin.toml names row {id}, which t-rows.toml does not"
            ));
        }
    }
    if !ci.contains(DARWIN_RECEIPT) {
        problems.push("the Darwin receipt does not read t-rows-darwin.toml".to_owned());
    }
    let darwin_it_rows: Vec<&str> = ci
        .lines()
        .filter(|line| line.contains("archive: bot-it-darwin"))
        .collect();
    if darwin_it_rows.is_empty()
        || darwin_it_rows
            .iter()
            .any(|row| !row.contains(DARWIN_ROW_FILTER))
    {
        problems.push("a Darwin it row runs more than the row-named tests".to_owned());
    }
    if !ci.contains(EVERY_TIER) {
        problems.push("the Linux saturation row does not run every tier".to_owned());
    }
    problems
}

/// The committed workflow and maps keep the suites on Linux, the counter
/// counts each runner form, and each way back onto macOS is named.
#[test]
fn the_platform_rows_run_only_the_platform_gates() -> TestResult {
    let ci = std::fs::read_to_string(root().join(WORKFLOW_REL))?;
    let full = std::fs::read_to_string(root().join("docs/acceptance/t-rows.toml"))?;
    let darwin = std::fs::read_to_string(root().join("docs/acceptance/t-rows-darwin.toml"))?;
    assert_eq!(platform_problems(&ci, &full, &darwin), Vec::<String>::new());
    // The eleven ADR 0042 counts: three seam rows (two lgwks_std slices and
    // the process binaries), four `_tNN` rows, three Apple storefront legs and
    // AppCUI's macOS backend.
    assert_eq!(macos_jobs(&ci), 11);
    let listed = ci.replacen(
        "os: [windows-latest, ubuntu-24.04-arm]",
        "os: [windows-latest, macos-15]",
        1,
    );
    assert_ne!(listed, ci, "the AppCUI os list must exist");
    assert_eq!(macos_jobs(&listed), 12, "a list item on macos-15 is a job");
    let suite_rows = "    runs-on: ubuntu-24.04-arm\n    container:\n      image: \"rust:1.99.1-trixie\"\n      options: \"--init --tmpfs /tmp:rw,exec,nosuid,size=6g\"\n    timeout-minutes: 5\n    strategy:";
    let on_macos = ci.replacen(
        suite_rows,
        &suite_rows.replacen("ubuntu-24.04-arm", "macos-15", 1),
        1,
    );
    assert_ne!(on_macos, ci, "the Linux suite rows must exist");
    assert!(
        platform_problems(&on_macos, &full, &darwin)
            .iter()
            .any(|problem| problem.starts_with("ci.yml starts ")),
        "the Linux suites moved to macOS pass the cap"
    );
    let controls = [
        (
            "ci",
            " --map docs/acceptance/t-rows-darwin.toml",
            "",
            "the Darwin receipt does not read t-rows-darwin.toml",
        ),
        (
            "ci",
            DARWIN_ROW_FILTER,
            "it_filter: \"binary(it)\"",
            "a Darwin it row runs more than the row-named tests",
        ),
        (
            "ci",
            EVERY_TIER,
            "sat_filter: \"binary(sim_review_path) & test(=saturation_r32_tier_10000)\"",
            "the Linux saturation row does not run every tier",
        ),
        (
            "darwin",
            "covers 1 of the 4 tests",
            "covers 2 of the 4 tests",
            "t-rows-darwin.toml's T01 gap does not say it covers 1 of the 4 tests",
        ),
        (
            "darwin",
            "id = \"T36\"",
            "id = \"T37\"",
            "t-rows-darwin.toml names row T37, which t-rows.toml does not",
        ),
    ];
    for (file, from, to, named) in controls {
        let (ci_text, darwin_text) = match file {
            "ci" => (ci.replacen(from, to, 1), darwin.clone()),
            _ => (ci.clone(), darwin.replacen(from, to, 1)),
        };
        assert!(
            ci_text != ci || darwin_text != darwin,
            "the text the control mutates must exist: {from}"
        );
        let problems = platform_problems(&ci_text, &full, &darwin_text);
        assert!(
            problems.iter().any(|problem| problem == named),
            "{named}: {problems:?}"
        );
    }
    let dropped = darwin.replacen(
        "  \"it::t_rows::a_one_declaration_one_call_returns_a_typed_value_t01\",\n",
        "",
        1,
    );
    assert_ne!(dropped, darwin, "the T01 test must be named");
    assert!(
        platform_problems(&ci, &full, &dropped)
            .iter()
            .any(|problem| problem.starts_with("t-rows-darwin.toml's T01 names ")),
        "a row-named test the Darwin map drops is named"
    );
    assert!(row_named("it::t_rows::x_t01") && !row_named("it::t_rows::x_t1") && !row_named("_t0x"));
    Ok(())
}
