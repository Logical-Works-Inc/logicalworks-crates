//! Why a generated lexicon page, or a guarantee a word makes, does not hold.

use std::fmt;

use super::{PAGE_LINES, PAGE_TOKENS, WRITE_PAGE};

/// One variant per failure, so a test reads the case and never parses a
/// message.
#[derive(Debug)]
pub(super) enum PageError {
    /// A kind of proof a newer `lgwks_ast` adds that has no citation form here.
    NoCitationForm(String),
    /// A kind of proof that has no resolver here.
    NoResolver(String),
    /// A word's example that the macro refuses.
    ExampleRefused { spelling: String, refusal: String },
    /// A word that guarantees nothing.
    GuaranteesNothing(String),
    /// A guarantee whose test is not found, and why.
    Unproven {
        spelling: String,
        claim: String,
        why: Box<PageError>,
    },
    /// A compile-fail case `REFUSALS` does not hold.
    NoCompileFailCase(String),
    /// Evidence naming a file that is not a simulation.
    NotSimulation(String),
    /// A simulation file with no test or swept family of that name.
    NoTest { file: String, test: String },
    /// A file that could not be read or written.
    Io { path: String, error: std::io::Error },
    /// One word's section alone is past the size law.
    SectionTooLong(String),
    /// A committed page that differs from the one the lexicon renders.
    Stale(String),
    /// A page past the size law.
    PageTooLong(String),
    /// A committed page the lexicon no longer renders.
    Leftover(String),
    /// Every page together is past the token budget.
    OverBudget(usize),
    /// A place a newer `lgwks_ast` adds that has no sentence here.
    NoSentence(String),
    /// A word missing from the pages, or out of table order.
    MissingWord(String),
    /// Crate front pages that do not open with the rendered lexicon.
    FrontPages(Vec<String>),
}

impl fmt::Display for PageError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoCitationForm(ref evidence) => write!(out, "no citation form for {evidence}"),
            Self::NoResolver(ref evidence) => write!(out, "no resolver for {evidence}"),
            Self::ExampleRefused {
                ref spelling,
                ref refusal,
            } => write!(out, "the example for `{spelling}` is refused: {refusal}"),
            Self::GuaranteesNothing(ref spelling) => {
                write!(out, "`{spelling}` guarantees nothing")
            }
            Self::Unproven {
                ref spelling,
                ref claim,
                ref why,
            } => write!(out, "`{spelling}` ({claim}): {why}"),
            Self::NoCompileFailCase(ref case) => {
                write!(out, "no compile-fail case named {case:?}")
            }
            Self::NotSimulation(ref file) => write!(out, "{file} is not a simulation file"),
            Self::NoTest { ref file, ref test } => {
                write!(out, "{file} has no test or swept family `{test}`")
            }
            Self::Io {
                ref path,
                ref error,
            } => write!(out, "{path}: {error}"),
            Self::SectionTooLong(ref word) => {
                write!(out, "the `{word}` section alone is past {PAGE_LINES} lines")
            }
            Self::Stale(ref name) => write!(
                out,
                "{name} is generated from LEXICON and differs from it; rerun this test \
                 with {WRITE_PAGE}=1 and commit the pages"
            ),
            Self::PageTooLong(ref name) => write!(out, "{name} is past {PAGE_LINES} lines"),
            Self::Leftover(ref name) => write!(
                out,
                "{name} is a page the lexicon no longer renders; delete it"
            ),
            Self::OverBudget(tokens) => write!(
                out,
                "the lexicon pages are {tokens} tokens (bytes / 4, rounded up), past {PAGE_TOKENS}"
            ),
            Self::NoSentence(ref position) => write!(out, "no sentence for {position}"),
            Self::MissingWord(ref word) => write!(out, "`{word}` is missing or out of order"),
            Self::FrontPages(ref found) => write!(
                out,
                "{}; rerun this test with {WRITE_PAGE}=1 and commit the fragments",
                found.join("; ")
            ),
        }
    }
}

impl std::error::Error for PageError {}

/// `error` on `path`, as a page error.
pub(super) fn io(path: impl Into<String>) -> impl FnOnce(std::io::Error) -> PageError {
    let path = path.into();
    move |error| PageError::Io { path, error }
}
