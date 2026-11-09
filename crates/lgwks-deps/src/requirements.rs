//! Checks that `REQUIREMENTS.md` has not drifted without a recorded supersession.
//!
//! The product requirements are the repository's normative spine. A requirement
//! is superseded, never edited, so the gate needs a memory of what each
//! normative sentence said the last time a person decided it. That memory is
//! `scripts/requirements.lock`: one entry per requirement id, holding a short
//! digest of its normative sentence and the sentence itself.
//!
//! The check answers one question: has any normative sentence changed since it
//! was last recorded, and if so, did someone say so out loud? A mismatch is not
//! silently repaired. `--update` rewrites the lock, but only for requirements
//! whose id appears in the Supersession log section of `REQUIREMENTS.md`.
//!
//! The spine may span pages (ADR 0023): `REQUIREMENTS.md`, then each
//! `docs/requirements/*.md` in name order, read as one text. An id declared
//! twice is refused, because the later copy would hide an edit to the earlier.
//!
//! Usage: `check-requirements [--update] [ROOT]`. The exit status is non-zero on
//! any mismatch or on a refused update.
//!
//! The one implementation of this check in the estate (#695): this file is the
//! rule, the `check-requirements` binary reads its arguments and runs
//! [`execute`], and `logical_ci`'s `governance` gate calls [`execute`] as a
//! library, so the requirement that a normative sentence moves only under a
//! Supersession log entry exists once with one owner.
//!
//! [`execute`]: crate::requirements::execute

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use digest::digest_of;

mod digest;
#[cfg(test)]
mod tests;

/// The lock's name as it is spelled in messages.
const LOCK_NAME: &str = "scripts/requirements.lock";

/// The directory whose Markdown pages continue `REQUIREMENTS.md`.
const PAGES_DIR: &str = "docs/requirements";

/// How many continuation pages one spine may hold; a tree is caller data.
const MAX_PAGES: usize = 64;

/// The lock's header comment, written verbatim ahead of its rows.
const LOCK_HEADER: &str = "\
# The normative sentence of each product requirement, as it read when a person
# last decided it. `REQUIREMENTS.md` is superseded, never edited, so this lock
# is the memory the gate checks an edit against.
#
# One entry per requirement: `R<id>` TAB `<sha256, first 12 hex>` TAB
# `<the normative sentence, whitespace-collapsed>`.
# Generated and checked by the `check-requirements` binary of `lgwks_deps`. A changed digest
# with no Supersession log entry naming that id is refused; record the
# supersession first, then `--update`.
";

/// Why the check could not run.
#[derive(Debug)]
enum CheckError {
    /// A file could not be read or written.
    Io {
        /// What was being touched.
        path: PathBuf,
        /// What the operating system said.
        cause: io::Error,
    },
    /// A requirement heading has no blockquote under it.
    NoBlockquote {
        /// The requirement id.
        id: u64,
    },
    /// A lock row is not `R<id> TAB digest TAB sentence`.
    BadLockRow {
        /// The one-based line number.
        line: usize,
    },
    /// A lock row's id is not `R<digits>`.
    BadLockId {
        /// The one-based line number.
        line: usize,
        /// The id as written.
        key: String,
    },
    /// The text to digest is longer than a SHA-256 length field can state.
    TooLong,
    /// Two headings declare one requirement.
    DeclaredTwice {
        /// The requirement id.
        id: u64,
    },
    /// The spine continues on more pages than [`MAX_PAGES`].
    TooManyPages {
        /// How many pages the directory holds.
        count: usize,
    },
}

impl fmt::Display for CheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io {
                ref path,
                ref cause,
            } => write!(formatter, "{}: {cause}", path.display()),
            Self::NoBlockquote { id } => write!(
                formatter,
                "REQUIREMENTS.md: R{id} has no normative blockquote. A requirement without a \
                 `>` sentence is not written yet."
            ),
            Self::BadLockRow { line } => write!(
                formatter,
                "{LOCK_NAME}:{line}: expected `R<id> TAB digest TAB sentence`"
            ),
            Self::BadLockId { line, ref key } => {
                write!(formatter, "{LOCK_NAME}:{line}: bad requirement id {key:?}")
            }
            Self::TooLong => write!(formatter, "the sentence is too long to digest"),
            Self::DeclaredTwice { id } => write!(
                formatter,
                "R{id} is declared twice across REQUIREMENTS.md and {PAGES_DIR}: the later copy \
                 would hide an edit to the earlier one"
            ),
            Self::TooManyPages { count } => write!(
                formatter,
                "{PAGES_DIR} holds {count} pages, past the ceiling of {MAX_PAGES}"
            ),
        }
    }
}

impl std::error::Error for CheckError {}

/// Logs `error` at the site that produced it, then hands it back as the `Err`.
fn fail<T>(error: CheckError, site: &'static str) -> Result<T, CheckError> {
    lgwks_std::trace::debug!(error = ?error, site, "returning an error to the caller");
    Err(error)
}

/// The requirements and the ids the Supersession log names.
struct Parsed {
    /// Each requirement's normative sentence by id.
    sentences: BTreeMap<u64, String>,
    /// Ids the Supersession log names: a receipt that the requirement moved.
    logged: BTreeSet<u64>,
}

/// One lock row: the stored digest and sentence.
struct Locked {
    /// The first twelve hex digits of the sentence's SHA-256.
    digest: String,
    /// The sentence as recorded.
    sentence: String,
}

/// What the comparison of the requirements with the lock found.
struct Findings {
    /// One message per mismatch.
    problems: Vec<String>,
    /// The ids that moved.
    changed: Vec<u64>,
}

/// The exit code a run settles on.
type Code = u8;

/// Reads a file whole, naming it when it cannot be read.
fn read(path: &Path) -> Result<String, CheckError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(cause) => fail(
            CheckError::Io {
                path: path.to_path_buf(),
                cause,
            },
            "read",
        ),
    }
}

/// The continuation pages under `root`, in name order. No directory is no pages,
/// so a repository that keeps one file reads as it always did.
fn continuation_pages(root: &Path) -> Result<Vec<PathBuf>, CheckError> {
    let directory = root.join(PAGES_DIR);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let unreadable = |cause: io::Error| CheckError::Io {
        path: directory.clone(),
        cause,
    };
    let reader = match fs::read_dir(&directory) {
        Ok(reader) => reader,
        Err(cause) => return fail(unreadable(cause), "continuation_pages"),
    };
    let mut pages = Vec::new();
    for entry in reader {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(cause) => return fail(unreadable(cause), "continuation_pages"),
        };
        if path.extension().is_some_and(|extension| extension == "md") {
            pages.push(path);
        }
    }
    if pages.len() > MAX_PAGES {
        let count = pages.len();
        return fail(CheckError::TooManyPages { count }, "continuation_pages");
    }
    pages.sort();
    Ok(pages)
}

/// The spine's whole text: `REQUIREMENTS.md`, then each continuation page.
fn read_spine(root: &Path, requirements_path: &Path) -> Result<String, CheckError> {
    let mut text = read(requirements_path)?;
    for page in continuation_pages(root)? {
        text.push('\n');
        text.push_str(&read(&page)?);
    }
    Ok(text)
}

/// A run of ASCII digits as a requirement id, or `None` when it does not fit.
fn parse_id(digits: &str) -> Option<u64> {
    match digits.parse::<u64>() {
        Ok(id) => Some(id),
        Err(error) => {
            lgwks_std::trace::debug!(%error, digits, "a requirement id does not parse as a u64");
            None
        }
    }
}

/// A requirement heading, `### R12. Title`, as its id.
fn heading_id(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("###")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start().strip_prefix('R')?;
    let end = match rest.find(|ch: char| !ch.is_ascii_digit()) {
        Some(end) => end,
        None => rest.len(),
    };
    let digits = rest.get(..end)?;
    if digits.is_empty() {
        return None;
    }
    let after = rest.get(end..)?.strip_prefix('.')?;
    if !after.starts_with(char::is_whitespace) || after.trim().is_empty() {
        return None;
    }
    parse_id(digits)
}

/// A blockquote line's body: the text after `>` and one optional space.
fn quote_body(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('>')?;
    match rest.chars().next() {
        Some(first) if first.is_whitespace() => rest.strip_prefix(first),
        _ => Some(rest),
    }
}

/// Whether `line` is the `## Supersession log` heading.
fn is_log_heading(line: &str) -> bool {
    match line.strip_prefix("##") {
        Some(rest) => rest.starts_with(char::is_whitespace) && rest.trim() == "Supersession log",
        None => false,
    }
}

/// Whether `ch` continues a word, for the `\b` boundaries of an id.
fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Every `R<digits>` id in `line` that stands alone as a word.
fn log_ids(line: &str) -> Vec<u64> {
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    for (start, ch) in line.char_indices() {
        let standalone_start = match previous {
            Some(before) => !is_word(before),
            None => true,
        };
        previous = Some(ch);
        if ch != 'R' || !standalone_start {
            continue;
        }
        let Some(tail) = line.get(start.saturating_add(1)..) else {
            continue;
        };
        let end = match tail.find(|digit: char| !digit.is_ascii_digit()) {
            Some(end) => end,
            None => tail.len(),
        };
        let Some(digits) = tail.get(..end) else {
            continue;
        };
        if digits.is_empty() {
            continue;
        }
        let bounded = match tail.get(end..).and_then(|rest| rest.chars().next()) {
            Some(after) => !is_word(after),
            None => true,
        };
        if !bounded {
            continue;
        }
        if let Some(id) = parse_id(digits) {
            found.push(id);
        }
    }
    found
}

/// Collapses a blockquote to one comparable sentence.
fn normative_sentence(block: &[&str]) -> String {
    block
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads each requirement's sentence and the ids the log names.
fn parse_requirements(text: &str) -> Result<Parsed, CheckError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut sentences = BTreeMap::new();
    let mut logged = BTreeSet::new();
    let mut index = 0_usize;
    while let Some(line) = lines.get(index) {
        if is_log_heading(line) {
            index = index.saturating_add(1);
            while let Some(entry) = lines.get(index) {
                if entry.starts_with("## ") {
                    break;
                }
                logged.extend(log_ids(entry));
                index = index.saturating_add(1);
            }
            continue;
        }
        let Some(id) = heading_id(line) else {
            index = index.saturating_add(1);
            continue;
        };
        index = index.saturating_add(1);
        while lines
            .get(index)
            .is_some_and(|blank| blank.trim().is_empty())
        {
            index = index.saturating_add(1);
        }
        let mut quote: Vec<&str> = Vec::new();
        while let Some(current) = lines.get(index) {
            if let Some(body) = quote_body(current) {
                quote.push(body);
                index = index.saturating_add(1);
                continue;
            }
            // A hard-wrapped quote continues over one blank line only when the
            // line after it is quoted again.
            if current.trim().is_empty()
                && lines
                    .get(index.saturating_add(1))
                    .is_some_and(|next| quote_body(next).is_some())
            {
                quote.push("");
                index = index.saturating_add(1);
                continue;
            }
            break;
        }
        if quote.is_empty() {
            return fail(CheckError::NoBlockquote { id }, "parse_requirements");
        }
        if sentences.insert(id, normative_sentence(&quote)).is_some() {
            return fail(CheckError::DeclaredTwice { id }, "parse_requirements");
        }
    }
    Ok(Parsed { sentences, logged })
}

/// A lock key, `R<digits>`, as its id.
fn lock_id(key: &str) -> Option<u64> {
    let digits = key.trim().strip_prefix('R')?;
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    parse_id(digits)
}

/// Reads the lock's rows by id.
fn parse_lock(text: &str) -> Result<BTreeMap<u64, Locked>, CheckError> {
    let mut entries = BTreeMap::new();
    for (offset, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let number = offset.saturating_add(1);
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        let (Some(key), Some(digest), Some(sentence)) = (parts.first(), parts.get(1), parts.get(2))
        else {
            return fail(CheckError::BadLockRow { line: number }, "parse_lock");
        };
        let Some(id) = lock_id(key) else {
            return fail(
                CheckError::BadLockId {
                    line: number,
                    key: (*key).to_owned(),
                },
                "parse_lock",
            );
        };
        entries.insert(
            id,
            Locked {
                digest: digest.trim().to_owned(),
                sentence: (*sentence).to_owned(),
            },
        );
    }
    Ok(entries)
}

/// Writes the lock from the current sentences.
fn write_lock(path: &Path, sentences: &BTreeMap<u64, String>) -> Result<(), CheckError> {
    let mut rows = vec![LOCK_HEADER.to_owned()];
    for (id, sentence) in sentences {
        rows.push(format!("R{id}\t{}\t{sentence}\n", digest_of(sentence)?));
    }
    match fs::write(path, rows.concat()) {
        Ok(()) => Ok(()),
        Err(cause) => fail(
            CheckError::Io {
                path: path.to_path_buf(),
                cause,
            },
            "write_lock",
        ),
    }
}

/// Compares the requirements with the lock.
fn compare(
    sentences: &BTreeMap<u64, String>,
    locked: &BTreeMap<u64, Locked>,
) -> Result<Findings, CheckError> {
    let mut problems = Vec::new();
    let mut changed = Vec::new();
    for (id, sentence) in sentences {
        let Some(old) = locked.get(id) else {
            problems.push(format!(
                "R{id}: present in REQUIREMENTS.md, absent from the lock"
            ));
            changed.push(*id);
            continue;
        };
        if digest_of(sentence)? != old.digest || *sentence != old.sentence {
            problems.push(format!(
                "R{id}: normative sentence changed\n    was: {}\n    now: {sentence}",
                old.sentence
            ));
            changed.push(*id);
        }
    }
    for id in locked.keys() {
        if !sentences.contains_key(id) {
            problems.push(format!(
                "R{id}: present in the lock, absent from REQUIREMENTS.md"
            ));
            changed.push(*id);
        }
    }
    Ok(Findings { problems, changed })
}

/// Runs the check over `root`, writing its verdict to the two handles.
#[doc(hidden)]
pub fn execute(
    root: &Path,
    update: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<Code, Box<dyn std::error::Error>> {
    let requirements_path = root.join("REQUIREMENTS.md");
    let lock_path = root.join("scripts").join("requirements.lock");
    if !requirements_path.is_file() {
        writeln!(err, "REQUIREMENTS.md is missing")?;
        return Ok(1);
    }
    let parsed = parse_requirements(&read_spine(root, &requirements_path)?)?;
    if parsed.sentences.is_empty() {
        writeln!(err, "REQUIREMENTS.md declares no requirements")?;
        return Ok(1);
    }
    if !lock_path.is_file() {
        if update {
            write_lock(&lock_path, &parsed.sentences)?;
            writeln!(
                out,
                "wrote {LOCK_NAME} — {} requirement(s)",
                parsed.sentences.len()
            )?;
            return Ok(0);
        }
        writeln!(
            err,
            "{LOCK_NAME} is missing. Run with --update to record the current text."
        )?;
        return Ok(1);
    }
    let locked = parse_lock(&read(&lock_path)?)?;
    let findings = compare(&parsed.sentences, &locked)?;
    let mut unlogged: Vec<u64> = findings
        .changed
        .iter()
        .copied()
        .filter(|id| !parsed.logged.contains(id))
        .collect();
    unlogged.sort_unstable();
    unlogged.dedup();
    if !findings.problems.is_empty() {
        for problem in &findings.problems {
            writeln!(err, "{problem}")?;
        }
        if !unlogged.is_empty() {
            let names: Vec<String> = unlogged.iter().map(|id| format!("R{id}")).collect();
            writeln!(
                err,
                "\n{} change(s) have no Supersession log entry: {}.\nA requirement is \
                 superseded, never edited. Add an entry to the Supersession log in \
                 REQUIREMENTS.md naming the id and why the previous statement was wrong or \
                 incomplete, then re-run with --update.",
                unlogged.len(),
                names.join(", ")
            )?;
            return Ok(1);
        }
        if update {
            write_lock(&lock_path, &parsed.sentences)?;
            writeln!(
                out,
                "updated {LOCK_NAME} — {} supersession(s) recorded",
                findings.changed.len()
            )?;
            return Ok(0);
        }
        writeln!(
            err,
            "\nEvery change has a Supersession log entry. Re-run with --update to record the \
             new text."
        )?;
        return Ok(1);
    }
    if update {
        writeln!(
            out,
            "{LOCK_NAME} already matches REQUIREMENTS.md; nothing to do"
        )?;
        return Ok(0);
    }
    writeln!(
        out,
        "requirements: {} declared, all digests match the lock",
        parsed.sentences.len()
    )?;
    Ok(0)
}
