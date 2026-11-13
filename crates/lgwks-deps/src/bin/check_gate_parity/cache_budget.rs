//! The cache budget (ADR 0041): main saves only the dependency builds that
//! set the 300 s wall, and `cache-budget.yml` prunes by last use after every
//! workflow that saves.
//!
//! Source: 44 current entries held 13.4 GB past GitHub's 10 GB, so each main
//! run evicted by last use an entry a long job needed, and the clippy, timing
//! and feature lanes logged "No cache found" three main runs in a row (runs
//! 43345895098, 43397802234, 43399257060).

use std::error::Error;
use std::path::{Path, PathBuf};

use crate::{OTHER_WORKFLOWS_REL, PAGES_REL, WORKFLOW_REL};

type TestResult = Result<(), Box<dyn Error>>;

/// The jobs that do not save their dependency build: each compiled within
/// 150 s cold on run 43397802234, half the 300 s wall, so none sets it. The
/// value is the `save-cache` input each passes at the `nth` use of its key; a
/// key not named here saves.
const NO_SAVE: [(&str, &str, usize, &str); 10] = [
    ("ci.yml", "gate", 0, "\"false\""),
    ("ci.yml", "bot-nodefault", 0, "\"false\""),
    ("ci.yml", "clippy-${{ matrix.part }}", 0, "\"false\""),
    ("ci.yml", "docs", 0, "\"false\""),
    (
        "ci.yml",
        "${{ matrix.cache }}",
        0,
        "${{ matrix.cache != 'storefront-deps' }}",
    ),
    ("ci.yml", "targets", 0, "\"false\""),
    ("ci.yml", "timing", 0, "\"false\""),
    (
        "ci.yml",
        "feature-${{ matrix.part }}",
        0,
        "${{ matrix.part == 'lgwks-ast' || matrix.part == 'docs.rs' }}",
    ),
    // The second `appcui` job is the Windows and Linux one.
    ("ci.yml", "appcui", 1, "${{ runner.os == 'Windows' }}"),
    ("pages.yml", "pages", 0, "\"false\""),
];

/// The repository this crate sits in.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Each `cache-key:` of `workflow` with the `save-cache:` value that follows
/// it past any comment lines; `None` is a job that saves by the default.
fn saves(workflow: &str) -> Vec<(&str, Option<&str>)> {
    workflow
        .split("cache-key: ")
        .skip(1)
        .filter_map(|after| {
            let (key, rest) = after.split_once('\n')?;
            let next = rest
                .lines()
                .map(str::trim_start)
                .find(|line| !line.starts_with('#'))?;
            Some((key, next.strip_prefix("save-cache: ")))
        })
        .collect()
}

/// Every way the committed workflows can lose the budget, by name.
fn cache_budget_problems(action: &str, workflows: &[(&str, &str)]) -> Vec<String> {
    let mut problems = Vec::new();
    if !action
        .contains("save-if: ${{ github.ref == 'refs/heads/main' && inputs.save-cache == 'true' }}")
    {
        problems.push("hosted-rust saves every job's build, whatever its cold time".to_owned());
    }
    let Some(&(_, budget)) = workflows
        .iter()
        .find(|&&(name, _)| name == "cache-budget.yml")
    else {
        problems.push("no cache-budget.yml prunes the cache".to_owned());
        return problems;
    };
    let saving: Vec<&str> = match budget
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("SAVING_WORKFLOWS: "))
    {
        Some(names) => names.split_whitespace().collect(),
        None => Vec::new(),
    };
    for &(name, text) in workflows {
        let mut seen: Vec<&str> = Vec::new();
        let mut saves_a_build = false;
        for (key, value) in saves(text) {
            let nth = seen.iter().filter(|other| **other == key).count();
            seen.push(key);
            let expected = NO_SAVE
                .iter()
                .find(|&&(file, no_save, at, _)| file == name && no_save == key && at == nth)
                .map(|&(_, _, _, value)| value);
            if value != expected {
                problems.push(format!(
                    "{name}'s {key} build saves on {value:?}, not {expected:?} (ADR 0041)"
                ));
            }
            saves_a_build |= value != Some("\"false\"");
        }
        if saves_a_build && !saving.contains(&name) {
            problems.push(format!(
                "cache-budget.yml does not prune after {name}, which saves a build"
            ));
        }
    }
    let after_each = saving
        .iter()
        .map(|file| file.trim_end_matches(".yml"))
        .collect::<Vec<_>>()
        .join(", ");
    if !budget.contains(&format!("workflows: [{after_each}]\n"))
        || !budget.contains("    branches: [main]\n")
        || !budget.contains("  actions: write\n")
    {
        problems.push("cache-budget.yml does not run after every saving workflow".to_owned());
    }
    if !budget.contains("select(.ref != \"refs/heads/main\"")
        || !budget.contains("(.last_accessed_at | stamp) < ($cutoff | stamp)")
    {
        problems.push("cache-budget.yml does not prune by last use".to_owned());
    }
    if !budget.contains("BUDGET_BYTES: \"9663676416\"")
        || !budget.contains("if [ \"${total}\" -gt \"${BUDGET_BYTES}\" ]; then")
    {
        problems.push("cache-budget.yml does not hold its 9 GiB budget".to_owned());
    }
    problems
}

/// The committed workflows by file name.
fn committed() -> std::io::Result<Vec<(String, String)>> {
    let mut workflows = Vec::new();
    for rel in [WORKFLOW_REL, PAGES_REL]
        .into_iter()
        .chain(OTHER_WORKFLOWS_REL)
    {
        let name = match rel.rsplit_once('/') {
            Some((_, name)) => name,
            None => rel,
        };
        workflows.push((name.to_owned(), std::fs::read_to_string(root().join(rel))?));
    }
    Ok(workflows)
}

/// `cache_budget_problems` over owned texts, with `file` changed by `swap`.
fn judge(
    action: &str,
    workflows: &[(String, String)],
    swap: Option<(&str, &str, &str)>,
) -> Vec<String> {
    let changed: Vec<(&str, String)> = workflows
        .iter()
        .map(|pair| match swap {
            Some((file, from, to)) if file == pair.0 => {
                (pair.0.as_str(), pair.1.replacen(from, to, 1))
            }
            _ => (pair.0.as_str(), pair.1.clone()),
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = changed
        .iter()
        .map(|&(name, ref text)| (name, text.as_str()))
        .collect();
    cache_budget_problems(action, &borrowed)
}

/// The committed workflows hold the budget, and each way of losing it is
/// named.
#[test]
fn the_cache_holds_only_the_builds_that_set_the_wall() -> TestResult {
    let action = std::fs::read_to_string(root().join(".github/actions/hosted-rust/action.yml"))?;
    let workflows = committed()?;
    assert_eq!(judge(&action, &workflows, None), Vec::<String>::new());
    let broken_action = action.replacen(" && inputs.save-cache == 'true'", "", 1);
    assert_ne!(broken_action, action, "the save condition must exist");
    assert_eq!(
        judge(&broken_action, &workflows, None),
        vec!["hosted-rust saves every job's build, whatever its cold time".to_owned()]
    );
    let controls = [
        (
            "ci.yml",
            "none saves (ADR 0041).\n          save-cache: \"false\"\n",
            "none saves (ADR 0041).\n",
            "ci.yml's clippy-${{ matrix.part }} build saves on None, not Some(\"\\\"false\\\"\") (ADR 0041)",
        ),
        (
            "ci.yml",
            "cache-key: ${{ matrix.it_group && 'bot-it' || matrix.archive }}\n",
            "cache-key: ${{ matrix.it_group && 'bot-it' || matrix.archive }}\n          save-cache: \"false\"\n",
            "ci.yml's ${{ matrix.it_group && 'bot-it' || matrix.archive }} build saves on Some(\"\\\"false\\\"\"), not None (ADR 0041)",
        ),
        (
            "ci.yml",
            "save-cache: ${{ runner.os == 'Windows' }}",
            "save-cache: \"false\"",
            "ci.yml's appcui build saves on Some(\"\\\"false\\\"\"), not Some(\"${{ runner.os == 'Windows' }}\") (ADR 0041)",
        ),
        (
            "pages.yml",
            "          save-cache: \"false\"\n",
            "",
            "pages.yml's pages build saves on None, not Some(\"\\\"false\\\"\") (ADR 0041)",
        ),
        (
            "cache-budget.yml",
            "workflows: [ci, logical_ci]",
            "workflows: [ci]",
            "cache-budget.yml does not run after every saving workflow",
        ),
        (
            "cache-budget.yml",
            "SAVING_WORKFLOWS: ci.yml logical_ci.yml",
            "SAVING_WORKFLOWS: ci.yml",
            "cache-budget.yml does not prune after logical_ci.yml, which saves a build",
        ),
        (
            "cache-budget.yml",
            "(.last_accessed_at | stamp) < ($cutoff | stamp)",
            "(.created_at | stamp) < ($cutoff | stamp)",
            "cache-budget.yml does not prune by last use",
        ),
        (
            "cache-budget.yml",
            "if [ \"${total}\" -gt \"${BUDGET_BYTES}\" ]; then",
            "if false; then",
            "cache-budget.yml does not hold its 9 GiB budget",
        ),
    ];
    for (file, from, to, named) in controls {
        let found = workflows
            .iter()
            .any(|pair| pair.0 == file && pair.1.contains(from));
        assert!(found, "the text the control mutates must exist: {from}");
        let problems = judge(&action, &workflows, Some((file, from, to)));
        assert!(
            problems.iter().any(|problem| problem == named),
            "{named}: {problems:?}"
        );
    }
    Ok(())
}
