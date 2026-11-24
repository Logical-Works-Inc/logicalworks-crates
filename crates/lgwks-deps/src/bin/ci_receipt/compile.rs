//! rustc and clippy diagnostics.
//!
//! A diagnostic opens with `error[E0425]: <text>` or `error: <text>` at the start
//! of a line, and names its place in a ` --> <file>:<line>:<col>` line within 6
//! lines. The lint behind a clippy error is named further down, in a `#[deny(..)]`
//! or `-D <lint>` note, inside the diagnostic's own block.
//!
//! The state is one pending diagnostic at most: it waits for its place, then for
//! its lint, and ends at a blank line, the next header, a `##[` command line or
//! when the window runs out. One without a place is no failure.

use super::failure::Kept;
use super::line::number;

/// The lines after a header within which its ` --> ` line must come.
const ARROW_WINDOW: usize = 6;

/// The lines after a header within which a lint note is still its own.
const NOTE_WINDOW: usize = 24;

/// Headers that sum a run up instead of naming a defect.
const SUMMARIES: [&str; 2] = ["aborting due to", "could not compile"];

/// One diagnostic with its place.
#[derive(Debug)]
pub(crate) struct CompileHit {
    /// The file it names.
    pub(crate) file: String,
    /// The line of that file.
    pub(crate) line: u32,
    /// The error code, else the lint name.
    pub(crate) code: Option<String>,
    /// The header's text.
    pub(crate) message: String,
    /// The command of the step running when it was read.
    pub(crate) command: Option<String>,
}

/// A diagnostic still being read.
#[derive(Debug)]
struct Pending {
    /// The header's text after `error: `.
    message: String,
    /// The bracketed error code, when the header had one.
    code: Option<String>,
    /// The lint a note named.
    lint: Option<String>,
    /// The place the ` --> ` line gave.
    site: Option<(String, u32)>,
    /// The lines read since the header.
    age: usize,
    /// The step running.
    command: Option<String>,
}

/// The diagnostic reader for one log.
#[derive(Debug, Default)]
pub(crate) struct Compile {
    /// The diagnostic being read.
    pending: Option<Pending>,
    /// The diagnostics read, in log order.
    hits: Vec<CompileHit>,
    /// The (file, line, code, message) of each kept, so a diagnostic printed once
    /// per target counts once, and whether one was dropped at the bound.
    kept: Kept<(String, u32, Option<String>, String)>,
}

impl Compile {
    /// Reads one cleaned line, with the command of the step it came in.
    pub(crate) fn feed(&mut self, line: &str, command: Option<&str>) {
        if let Some((code, message)) = header(line) {
            self.end();
            if !SUMMARIES.iter().any(|prefix| message.starts_with(prefix)) {
                self.pending = Some(Pending {
                    message: message.to_owned(),
                    code,
                    lint: None,
                    site: None,
                    age: 0,
                    command: command.map(str::to_owned),
                });
            }
            return;
        }
        if line.starts_with("warning: ") || line.starts_with("warning[") {
            self.end();
            return;
        }
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.read(line) {
            self.end();
        }
    }

    /// Keeps the pending diagnostic when it found its place, and drops it.
    fn end(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let Some((file, line)) = pending.site else {
            return;
        };
        let code = pending.code.or(pending.lint);
        let key = (file.clone(), line, code.clone(), pending.message.clone());
        if !self.kept.admit(key, "compile") {
            return;
        }
        self.hits.push(CompileHit {
            file,
            line,
            code,
            message: pending.message,
            command: pending.command,
        });
    }

    /// Every diagnostic read, and whether any was dropped.
    pub(crate) fn finish(mut self) -> (Vec<CompileHit>, bool) {
        self.end();
        (self.hits, self.kept.shed)
    }
}

impl Pending {
    /// Reads a line under the header; true when the diagnostic ends with it,
    /// complete or not.
    fn read(&mut self, line: &str) -> bool {
        self.age = self.age.saturating_add(1);
        if line.trim().is_empty() || line.starts_with("##[") {
            return true;
        }
        if self.site.is_none() {
            self.site = arrow(line);
            return self.site.is_none() && self.age >= ARROW_WINDOW;
        }
        if self.lint.is_none() {
            self.lint = lint_in(line);
        }
        self.age >= NOTE_WINDOW
    }
}

/// The error code and text of an `error[E0425]: ..` or `error: ..` header.
fn header(line: &str) -> Option<(Option<String>, &str)> {
    let rest = line.strip_prefix("error")?;
    if let Some(text) = rest.strip_prefix(": ") {
        return Some((None, text.trim()));
    }
    let (code, text) = rest.strip_prefix('[')?.split_once("]: ")?;
    if code.is_empty() || !code.chars().all(|found| found.is_ascii_alphanumeric()) {
        return None;
    }
    Some((Some(code.to_owned()), text.trim()))
}

/// The file and line of a ` --> <file>:<line>:<col>` line.
fn arrow(line: &str) -> Option<(String, u32)> {
    let site = line.trim().strip_prefix("--> ")?;
    let mut parts = site.trim().rsplitn(3, ':');
    number(parts.next()?)?;
    let row = number(parts.next()?)?;
    let file = parts.next()?;
    if file.is_empty() {
        return None;
    }
    Some((file.to_owned(), row))
}

/// The lint a `#[deny(<lint>)]` or `-D <lint>` note names.
fn lint_in(line: &str) -> Option<String> {
    if let Some((_, after)) = line.split_once("#[deny(") {
        return lint_name(after);
    }
    line.match_indices("-D ")
        .filter_map(|found| line.get(found.0.saturating_add(3)..))
        .find_map(lint_name)
}

/// The lint name that opens `text`, but not the `warnings` group.
fn lint_name(text: &str) -> Option<String> {
    let name: String = text
        .chars()
        .take_while(|found| found.is_ascii_alphanumeric() || matches!(*found, '_' | '-' | ':'))
        .collect();
    if name.is_empty() || name == "warnings" {
        return None;
    }
    Some(name)
}
