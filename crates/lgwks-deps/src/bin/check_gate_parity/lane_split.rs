//! Rule 6: one home per moved test.
//!
//! The scaffolded-app journeys left the workspace `tests` lane for the
//! `app-builds` lane, which CI runs as `apps` rows of the `tests_linux` matrix
//! (ADR 0028). A selector the workspace lane stops excluding runs twice; one no
//! `apps` row carries never runs in CI. Both are silent on each side alone.

use std::collections::BTreeSet;

use super::reader::read_manifest;

/// The lane whose tests the split moved out of the workspace lane.
pub(super) const MOVED_LANE: &str = "app-builds";

/// The workspace lane the moved tests left.
pub(super) const LEFT_LANE: &str = "tests";

/// Where the workspace lane's exclusion of the moved tests begins.
const EXCLUSION: &str = "not (package(lgwks_deps)";

/// The text that opens an `apps` row's filter in the workflow matrix.
const ROW_FILTER: &str = "apps_filter: \"";

/// Every `test(...)` selector of a nextest filter, as written, with the
/// parentheses of a regex inside it balanced.
pub(super) fn test_selectors(filter: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = filter;
    while let Some(start) = rest.find("test(") {
        let Some(tail) = rest.get(start..) else {
            break;
        };
        let mut depth = 0_usize;
        let mut end = None;
        for (at, cell) in tail.char_indices() {
            if cell == '(' {
                depth = depth.saturating_add(1);
            } else if cell == ')' {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    end = Some(at);
                    break;
                }
            }
        }
        let Some(end) = end else {
            break;
        };
        if let Some(selector) = tail.get(..=end) {
            found.push(selector.to_owned());
        }
        match tail.get(end.saturating_add(1)..) {
            Some(after) => rest = after,
            None => break,
        }
    }
    found
}

/// The filter an `apps` row of the workflow matrix runs, if `line` is one.
fn row_filter(line: &str) -> Option<&str> {
    if !line.contains("suite: apps") {
        return None;
    }
    let start = line.find(ROW_FILTER)?.saturating_add(ROW_FILTER.len());
    let rest = line.get(start..)?;
    rest.get(..rest.find('"')?)
}

/// The rule-6 violations: a test the `app-builds` lane selects that the
/// `tests` lane does not exclude, or that no `apps` row of the workflow runs.
pub(super) fn split_problems(manifest_text: &str, workflow_text: &str) -> Vec<String> {
    let manifest = read_manifest(manifest_text);
    let command = |id: &str| {
        manifest
            .lanes
            .iter()
            .find(|lane| lane.id == id)
            .and_then(|lane| lane.command.clone())
    };
    let Some(moved_command) = command(MOVED_LANE) else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    let moved = test_selectors(&moved_command);
    if moved.is_empty() {
        problems.push(format!("{MOVED_LANE}: its command selects no test"));
    }
    let left = command(LEFT_LANE);
    let excluded = match left.as_deref().and_then(|text| {
        text.find(EXCLUSION)
            .and_then(|at| text.get(at..).map(test_selectors))
    }) {
        Some(excluded) => excluded,
        None => {
            problems.push(format!(
                "{LEFT_LANE}: its command carries no `{EXCLUSION} ...)` for the tests {MOVED_LANE} runs"
            ));
            Vec::new()
        }
    };
    let rows: BTreeSet<String> = workflow_text
        .lines()
        .filter_map(row_filter)
        .flat_map(test_selectors)
        .collect();
    for selector in &moved {
        if !excluded.contains(selector) {
            problems.push(format!(
                "{LEFT_LANE} does not exclude {selector}, which {MOVED_LANE} runs: it would run twice"
            ));
        }
        if !rows.contains(selector) {
            problems.push(format!(
                "no apps row of ci.yml runs {selector}, which {LEFT_LANE} excludes: CI would never run it"
            ));
        }
    }
    problems
}
