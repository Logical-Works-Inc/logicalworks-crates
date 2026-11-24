//! nextest's result lines and the output block under each failed test.
//!
//! A result line reads `FAIL [   0.041s] ( 95/371) <binary> <test>`, with an
//! optional `TRY <n>` in front. The lines after a failing one, up to the next
//! result line, hold the test's output: the `panicked at <file>:<line>:<col>:`
//! line and the message under it, a `churn-slot:` or `test left` line, a seed.
//!
//! The state is one open block at most. A result line closes it, and opens the
//! next one when it is a failure not seen before. After the `Summary` line nextest
//! prints each failure again with no output under it, so the same key
//! (binary, test) is kept once, and a failure seen only there still counts.

use super::failure::Kept;
use super::line::number;

/// The output lines kept under one failed test.
const MAX_BLOCK_LINES: usize = 400;

/// The message lines read under a `panicked at` line: the message and three more.
const PANIC_LINES: usize = 4;

/// The `churn-slot:` and `test left` lines added to a message.
const NOTE_LINES: usize = 3;

/// The most digits of a seed: a `u64` has 20.
const SEED_DIGITS: usize = 20;

/// A failed test and what its output block said.
#[derive(Debug)]
pub(crate) struct TestHit {
    /// The nextest binary id, `lgwks_ast::it`.
    pub(crate) binary: String,
    /// The test's name within that binary.
    pub(crate) test: String,
    /// The file the panic named.
    pub(crate) file: Option<String>,
    /// The line the panic named.
    pub(crate) line: Option<u32>,
    /// The panic text and the orphan notes, or the status when there is neither.
    pub(crate) message: String,
    /// The seed the output printed.
    pub(crate) seed: Option<String>,
    /// The command of the step running when the failure was read.
    pub(crate) command: Option<String>,
}

/// A failure whose output block is still being read.
#[derive(Debug)]
struct Open {
    /// The nextest binary id.
    binary: String,
    /// The test's name.
    test: String,
    /// The status word, `FAIL` or `TIMEOUT`.
    label: String,
    /// The step running.
    command: Option<String>,
    /// The output lines so far.
    lines: Vec<String>,
}

/// One nextest result line.
struct Status<'line> {
    /// The status word.
    label: &'line str,
    /// The binary and test it names, when it names them.
    result: Option<(&'line str, &'line str)>,
}

/// What an output block said.
struct Block {
    /// The file of the first panic.
    file: Option<String>,
    /// The line of the first panic.
    line: Option<u32>,
    /// The message.
    message: String,
    /// The seed.
    seed: Option<String>,
}

/// The nextest reader for one log.
#[derive(Debug, Default)]
pub(crate) struct Nextest {
    /// The (binary, test) of every failure kept, and whether one was dropped
    /// at the bound.
    kept: Kept<(String, String)>,
    /// The failure whose output is being read.
    open: Option<Open>,
    /// The failures read, in log order.
    hits: Vec<TestHit>,
    /// Whether the `Summary` line has passed in the run being read.
    summary: bool,
}

impl Nextest {
    /// Reads one cleaned line, with the command of the step it came in.
    pub(crate) fn feed(&mut self, line: &str, command: Option<&str>) {
        if let Some(status) = status(line) {
            self.close();
            if failing(status.label)
                && let Some((binary, test)) = status.result
            {
                self.begin(binary, test, status.label, command);
            }
            return;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("Summary [") {
            self.close();
            self.summary = true;
        } else if trimmed.starts_with("Nextest run ID") {
            self.close();
            self.summary = false;
        } else if trimmed.starts_with("Cancelling ") || is_rule(trimmed) {
            self.close();
        } else if let Some(open) = self.open.as_mut()
            && open.lines.len() < MAX_BLOCK_LINES
        {
            open.lines.push(line.to_owned());
        }
    }

    /// Opens a failure unless it was seen already or the log has too many.
    fn begin(&mut self, binary: &str, test: &str, label: &str, command: Option<&str>) {
        if !self
            .kept
            .admit((binary.to_owned(), test.to_owned()), "nextest")
        {
            return;
        }
        let open = Open {
            binary: binary.to_owned(),
            test: test.to_owned(),
            label: label.to_owned(),
            command: command.map(str::to_owned),
            lines: Vec::new(),
        };
        if self.summary {
            self.keep(open);
        } else {
            self.open = Some(open);
        }
    }

    /// Ends the open block, if there is one.
    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            self.keep(open);
        }
    }

    /// Reads a finished block into a failure.
    fn keep(&mut self, open: Open) {
        let block = read_block(&open.lines, &open.label);
        self.hits.push(TestHit {
            binary: open.binary,
            test: open.test,
            file: block.file,
            line: block.line,
            message: block.message,
            seed: block.seed,
            command: open.command,
        });
    }

    /// Every failure read, and whether any was dropped.
    pub(crate) fn finish(mut self) -> (Vec<TestHit>, bool) {
        self.close();
        (self.hits, self.kept.shed)
    }
}

/// Whether `label` is a status word of a test that did not pass.
fn failing(label: &str) -> bool {
    matches!(
        label,
        "FAIL" | "TIMEOUT" | "LEAK" | "LEAK-FAIL" | "EXEC-FAIL"
    ) || label.starts_with("SIG")
}

/// Whether `label` is an upper-case status word such as `PASS` or `LEAK-FAIL`.
fn is_label(label: &str) -> bool {
    label
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
        && label
            .chars()
            .all(|found| found.is_ascii_uppercase() || found.is_ascii_digit() || found == '-')
}

/// Whether `text` is nextest's horizontal rule.
fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|found| found == '\u{2500}')
}

/// Reads `line` as a nextest result line, with any `TRY <n>` in front.
fn status(line: &str) -> Option<Status<'_>> {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("TRY ") {
        let (digits, tail) = after.split_once(' ')?;
        number(digits)?;
        rest = tail.trim_start();
    }
    let (label, tail) = rest.split_once(' ')?;
    if !is_label(label) {
        return None;
    }
    let (_, after) = tail.trim_start().strip_prefix('[')?.split_once(']')?;
    let after = after.trim_start();
    let named = match after.strip_prefix('(') {
        Some(counted) => counted
            .split_once(')')
            .map(|counted_and_name| counted_and_name.1.trim_start()),
        None => Some(after),
    };
    let result = named
        .and_then(|text| text.split_once(char::is_whitespace))
        .map(|pair| (pair.0, pair.1.trim()))
        .filter(|pair| !pair.0.is_empty() && !pair.1.is_empty());
    Some(Status { label, result })
}

/// The file and line of a `thread '..' panicked at <file>:<line>:<col>:` line.
fn panic_site(text: &str) -> Option<(String, u32)> {
    let (_, site) = text.split_once(" panicked at ")?;
    let site = site.trim().strip_suffix(':')?;
    let mut parts = site.rsplitn(3, ':');
    number(parts.next()?)?;
    let line = number(parts.next()?)?;
    let file = parts.next()?;
    if file.is_empty() {
        return None;
    }
    Some((file.to_owned(), line))
}

/// Whether `text` ends the message under a panic line.
fn ends_message(text: &str) -> bool {
    text.is_empty()
        || text.starts_with("note:")
        || text.starts_with("stack backtrace")
        || text.starts_with("thread '")
}

/// The digits after the first `seed` followed by one or more of ` `, `=`, `:`.
fn seed_in(text: &str) -> Option<String> {
    // The character before a match: the end of the text in front of it, or,
    // when that is empty, the `d` of the match before it.
    let mut previous = None;
    let mut rest = text;
    while let Some((front, after)) = rest.split_once("seed") {
        let before = front.chars().next_back().or(previous);
        previous = Some('d');
        rest = after;
        if before.is_some_and(|found| found.is_alphanumeric() || found == '_') {
            continue;
        }
        let value = after.trim_start_matches([' ', '=', ':']);
        if value.len() == after.len() {
            continue;
        }
        let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
        let next = value
            .get(digits.len()..)
            .and_then(|tail| tail.chars().next());
        let glued = next.is_some_and(|after| after.is_alphanumeric() || after == '_');
        if digits.is_empty() || digits.len() > SEED_DIGITS || glued {
            continue;
        }
        return Some(digits);
    }
    None
}

/// What the output lines under a failed test said.
fn read_block(lines: &[String], label: &str) -> Block {
    let mut site: Option<(String, u32, usize)> = None;
    let mut seed = None;
    let mut notes: Vec<&str> = Vec::new();
    let mut trailer = None;
    for (at, text) in lines.iter().enumerate() {
        if site.is_none()
            && let Some((file, line)) = panic_site(text)
        {
            site = Some((file, line, at));
        }
        if seed.is_none() {
            seed = seed_in(text);
        }
        let trimmed = text.trim();
        if (trimmed.contains("churn-slot:") || trimmed.contains("test left"))
            && notes.len() < NOTE_LINES
        {
            notes.push(trimmed);
        }
        if trailer.is_none() && trimmed.starts_with("(test ") && trimmed.ends_with(')') {
            trailer = Some(trimmed.trim_start_matches('(').trim_end_matches(')'));
        }
    }
    let mut parts: Vec<&str> = match site {
        Some((_, _, at)) => lines
            .iter()
            .skip(at.saturating_add(1))
            .map(|text| text.trim())
            .take_while(|text| !ends_message(text))
            .take(PANIC_LINES)
            .collect(),
        None => Vec::new(),
    };
    for note in notes {
        if !parts.contains(&note) {
            parts.push(note);
        }
    }
    let message = if !parts.is_empty() {
        parts.join("\n")
    } else if let Some(text) = trailer {
        text.to_owned()
    } else {
        label.to_owned()
    };
    let (file, line) = match site {
        Some((file, line, _)) => (Some(file), Some(line)),
        None => (None, None),
    };
    Block {
        file,
        line,
        message,
        seed,
    }
}
