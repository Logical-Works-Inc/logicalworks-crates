//! Rule 12: every `curl` a workflow or an action runs has a deadline.
//!
//! `curl` waits as long as a server holds the connection open. On run
//! 45910843702 one job's fetch of the kache release hung 120 s before GitHub
//! answered 504, and the run walled at 323 s against its 300 s limit with every
//! test green. A fetch with `--max-time` fails inside its bound, so its retry
//! starts at once. Every file under `.github/workflows` and every
//! `.github/actions/*/action.yml` is read, so a new workflow is held the day it
//! lands. Lines are read with their numbers, so `logical_lines` (which drops
//! them) is not this reader. The cases are in `bounded_fetch/sim_bounded_fetch.rs`.

#[cfg(test)]
mod sim_bounded_fetch;

use std::fs;
use std::path::Path;

use crate::{ParityError, fail, read};

/// The flags that bound a whole `curl` transfer.
const DEADLINES: [&str; 2] = ["--max-time", "-m"];

/// `line` without a YAML or shell comment: a whole-line `#`, or a ` #` that
/// opens a word.
fn code(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    match line.split_once(" #") {
        Some((before, _)) => before,
        None => line,
    }
}

/// Whether a word bounds the transfer it belongs to.
fn is_deadline(word: &str) -> bool {
    DEADLINES.contains(&word) || word.starts_with("--max-time=")
}

/// Whether one command of a list or pipeline runs a `curl` with no deadline.
/// Each `curl` word opens its own command, so one bound never covers the next.
fn unbounded(segment: &str) -> bool {
    let mut open = false;
    for word in segment
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | '$' | '"' | '\''))
        .filter(|word| !word.is_empty())
    {
        if word == "curl" {
            if open {
                return true;
            }
            open = true;
        } else if open && is_deadline(word) {
            open = false;
        }
    }
    open
}

/// The rule-12 violations of `text`, the file at `rel`: one per logical line
/// (backslash continuations joined) that holds a `curl` with no deadline,
/// named by the line it starts on.
pub(crate) fn problems(rel: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        let mut command = code(line).to_owned();
        while command.trim_end().ends_with('\\') {
            let Some((_, next)) = lines.next() else {
                break;
            };
            command = format!(
                "{} {}",
                command.trim_end().trim_end_matches('\\'),
                code(next)
            );
        }
        if command.split(['|', ';', '&']).any(unbounded) {
            found.push(format!(
                "{rel}:{} has a `curl` with no `--max-time`, so a stalled server holds the job \
                 past its wall (run 45910843702); bound every fetch",
                index.saturating_add(1)
            ));
        }
    }
    found
}

/// The names in `root`/`rel`, sorted so the verdict is the same on every host.
fn names(root: &Path, rel: &str) -> Result<Vec<String>, ParityError> {
    let dir = root.join(rel);
    let listing = match fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(cause) => return fail(ParityError::Io { path: dir, cause }, "bounded_fetch::names"),
    };
    let mut names = Vec::new();
    for entry in listing {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Err(cause) => {
                return fail(ParityError::Io { path: dir, cause }, "bounded_fetch::names");
            }
        }
    }
    names.sort();
    Ok(names)
}

/// The rule-12 violations of every workflow and action committed under `root`.
pub(crate) fn committed_problems(root: &Path) -> Result<Vec<String>, ParityError> {
    let mut files: Vec<String> = names(root, ".github/workflows")?
        .into_iter()
        .filter(|name| name.ends_with(".yml") || name.ends_with(".yaml"))
        .map(|name| format!(".github/workflows/{name}"))
        .collect();
    for action in names(root, ".github/actions")? {
        files.extend(
            ["action.yml", "action.yaml"]
                .map(|name| format!(".github/actions/{action}/{name}"))
                .into_iter()
                .filter(|rel| root.join(rel).is_file()),
        );
    }
    let mut found = Vec::new();
    for rel in files {
        found.extend(problems(&rel, &read(&root.join(&rel))?));
    }
    Ok(found)
}
