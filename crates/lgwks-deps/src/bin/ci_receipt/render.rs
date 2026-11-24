//! The receipt as JSON and as a markdown table.
//!
//! The JSON is written through the estate's own serde path (`lgwks_std::json`),
//! with the keys in the order the schema names them, which a `Value` tree would
//! sort. The markdown is a PR comment: every cell is cut and escaped, because the
//! text came from a log the code under test wrote.

use lgwks_std::json::serde::ser::{Serialize, SerializeStruct, Serializer};
use lgwks_std::json::{self};

use super::error::ReceiptError;
use super::failure::{Failure, Receipt};
use super::line::clip;

/// The most characters of a job, a place or a test in a table cell.
const CELL_CHARS: usize = 120;

/// The most characters of a repro in a table cell.
const REPRO_CELL_CHARS: usize = 300;

/// The schema number the receipt carries.
const SCHEMA: u32 = 1;

impl Serialize for Failure {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Failure", 11)?;
        state.serialize_field("job", &self.job)?;
        state.serialize_field("kind", self.kind.name())?;
        state.serialize_field("test", &self.test)?;
        state.serialize_field("binary", &self.binary)?;
        state.serialize_field("file", &self.file)?;
        state.serialize_field("line", &self.line)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("seed", &self.seed)?;
        state.serialize_field("message", &self.message)?;
        state.serialize_field("repro", &self.repro)?;
        state.serialize_field("step", &self.step)?;
        state.end()
    }
}

/// The receipt document: schema, failures, truncated, in that order.
struct Document<'receipt> {
    /// The failures kept.
    failures: &'receipt [Failure],
    /// Whether the logs held more.
    truncated: bool,
}

impl Serialize for Document<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Receipt", 3)?;
        state.serialize_field("schema", &SCHEMA)?;
        state.serialize_field("failures", self.failures)?;
        state.serialize_field("truncated", &self.truncated)?;
        state.end()
    }
}

/// The receipt as one line of JSON and a newline.
pub(crate) fn json(receipt: &Receipt) -> Result<String, ReceiptError> {
    let document = Document {
        failures: &receipt.failures,
        truncated: receipt.truncated(),
    };
    let mut text = json::to_string(&document).map_err(ReceiptError::Render)?;
    text.push('\n');
    Ok(text)
}

/// The receipt as a PR comment: a heading, one table row per failure, and a line
/// saying what was cut.
pub(crate) fn markdown(receipt: &Receipt, max: usize) -> String {
    let mut lines = vec!["### CI failure receipt".to_owned(), String::new()];
    if receipt.failures.is_empty() {
        lines.push("No failure was found in the logs.".to_owned());
    } else {
        lines.push("| job | kind | location | test or code | repro |".to_owned());
        lines.push("|---|---|---|---|---|".to_owned());
        lines.extend(receipt.failures.iter().map(row));
    }
    if receipt.cut > 0 {
        lines.push(String::new());
        lines.push(format!(
            "Truncated at --max {max}; failures cut: {}.",
            receipt.cut
        ));
    }
    if receipt.shed {
        lines.push(String::new());
        lines.push("Some log text lay past a read bound and was not read.".to_owned());
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// One table row.
fn row(failure: &Failure) -> String {
    let location = match (failure.file.as_deref(), failure.line) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (Some(file), None) => file.to_owned(),
        (None, _) => String::new(),
    };
    let what = match failure.test.as_deref().or(failure.code.as_deref()) {
        Some(found) => cell(found),
        None => cell(""),
    };
    format!(
        "| {} | {} | {} | {} | {} |",
        cell(&failure.job),
        failure.kind.name(),
        cell(&location),
        what,
        code_span(&failure.repro)
    )
}

/// `text` as a table cell: cut, on one line, with `\` and `|` escaped and no mention of
/// anyone (a zero-width space after each `@`), or `-` when empty.
fn cell(text: &str) -> String {
    if text.is_empty() {
        return "-".to_owned();
    }
    clip(text, CELL_CHARS)
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
        .replace('@', "@\u{200b}")
}

/// `text` as a code span a table cell holds, or `-` when empty: cut with `...`,
/// on one line, with `|` escaped, fenced by one more backtick than its longest run.
fn code_span(text: &str) -> String {
    if text.is_empty() {
        return "-".to_owned();
    }
    let cut = text.chars().count() > REPRO_CELL_CHARS;
    let mut shown = clip(text, REPRO_CELL_CHARS);
    if cut {
        shown.push_str("...");
    }
    let shown = shown.replace('|', "\\|").replace(['\n', '\r'], " ");
    let longest = shown
        .split(|found: char| found != '`')
        .fold(0_usize, |most, run| most.max(run.len()));
    if longest == 0 {
        return format!("`{shown}`");
    }
    let fence = "`".repeat(longest.saturating_add(1));
    format!("{fence} {shown} {fence}")
}
