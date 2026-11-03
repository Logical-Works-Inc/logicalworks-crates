//! The COBOL source reader: fixed and free format with COPY expansion (#777).
//!
//! Fixed format reads columns 8-72 as code: columns 1-6 are the sequence
//! area and are ignored, column 7 is the indicator (`*` and `/` comment the
//! line out, `-` continues the previous line, `D` reads as code under the
//! WITH DEBUGGING MODE reading), and column 73 and past are ignored. Free
//! format starts at a `>>SOURCE FORMAT IS FREE` or `>>SOURCE FORMAT FREE`
//! directive and reads every line whole. A `*>` marker ends the code on any
//! line, in either format.
//!
//! The line reader lives in `reader`, the statement classifiers in
//! `classify`: this module owns the program shape and its assembly.
//!
//! Statements end at a period followed by blank space or the line end, so a
//! `9.99` picture or a `1.5` literal never splits one. An `EXEC` block runs to
//! its `END-EXEC`, whatever periods sit inside. COPY expansion splices the
//! resolver's text at the COPY statement through `==` to `==` REPLACING pairs
//! only: any other REPLACING form leaves the copy unresolved rather than
//! half-applied. Names fold to uppercase, the way the language compares them.
//!
//! What the reader records, per program: the PROGRAM-ID, the paragraphs and
//! sections of the PROCEDURE DIVISION, and one node per statement for PERFORM
//! (with THRU, UNTIL, VARYING and inline), CALL (a literal, or a dynamic call
//! whose target is unknown), GO TO, OPEN, READ, WRITE, REWRITE, DELETE,
//! CLOSE, SELECT with ASSIGN, FD records, EXEC SQL SELECT, INSERT, UPDATE and
//! DELETE with their table names, EXEC CICS LINK and XCTL with their program,
//! and MOVE, COMPUTE, ADD and SUBTRACT with their targets. Any other line is
//! an `Other` statement node: kept with its lines, never skipped, and
//! never claimed to mean anything. Statements expanded from a copybook carry
//! the COPY statement's own line, the line that brought them.

/// The statement classifiers: one recorded kind per statement.
mod classify;
/// The line reader: format handling, statement assembly and COPY expansion.
mod reader;

use super::dialect::{Dialect, UnknownDirective, detect_dialect};
use super::error::LegacyError;
use super::{LegacyLimits, stripped, uppercased};

use reader::{read_statements, to_logical_lines};

/// Words that open a statement, so a bare `EXIT.` is never read as a paragraph.
const STATEMENT_WORDS: [&str; 28] = [
    "ACCEPT", "ADD", "ALTER", "CALL", "CANCEL", "CLOSE", "COMPUTE", "CONTINUE", "COPY", "DELETE",
    "DISPLAY", "DIVISION", "END", "EVALUATE", "EXEC", "EXIT", "FD", "GO", "GOBACK", "IF", "MOVE",
    "NEXT", "OPEN", "PERFORM", "READ", "RETURN", "STOP", "WRITE",
];

/// Words that start a division, ending the previous one.
const DIVISION_WORDS: [&str; 4] = ["IDENTIFICATION", "ENVIRONMENT", "DATA", "PROCEDURE"];

/// A COBOL program as read: its name, its paragraphs, and one node per
/// statement in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CobolProgram {
    /// The PROGRAM-ID, or `UNNAMED` when none names one.
    pub(crate) name: String,
    /// The paragraphs and sections of the PROCEDURE DIVISION, in order.
    pub(crate) paragraphs: Vec<Paragraph>,
    /// One node per statement, in source order.
    pub(crate) statements: Vec<Statement>,
    /// Every COPY the source names, resolved or recorded as a gap.
    pub(crate) copies: Vec<CopyRef>,
    /// The directive-shaped lines no dialect owns, with their lines.
    pub(crate) unknown_directives: Vec<UnknownDirective>,
    /// The dialect the directives name, or unknown on a tie or no hint.
    pub(crate) dialect: Dialect,
}

/// One paragraph or section of the PROCEDURE DIVISION.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Paragraph {
    /// The paragraph or section name, folded to uppercase.
    pub(crate) name: String,
    /// The section name, when the line declares one.
    pub(crate) section: Option<String>,
    /// The lines the declaration spans.
    pub(crate) lines: super::LineRange,
}

/// One statement node: its kind, its lines, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Statement {
    /// What the statement does, or [`StatementKind::Other`] when the reader
    /// claims nothing about it.
    pub(crate) kind: StatementKind,
    /// The lines the statement spans, attributed to the COPY that brought it.
    pub(crate) lines: super::LineRange,
    /// The statement text, lines joined with one blank.
    pub(crate) text: String,
}

/// What a statement does. Every variant carries the names the graphs read, in
/// full: a dynamic call carries no target rather than a guessed one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum StatementKind {
    /// `PERFORM name [THRU end] [UNTIL | VARYING | TIMES]`, or an inline
    /// PERFORM with no target.
    Perform {
        /// The performed paragraph, or `None` for an inline PERFORM.
        target: Option<String>,
        /// The THRU end, when one is named.
        thru: Option<String>,
        /// Whether UNTIL, VARYING, TIMES or TEST conditions it.
        conditional: bool,
    },
    /// `CALL literal` or `CALL variable`: a named target, or a dynamic call
    /// whose target is unknown.
    Call {
        /// The called program, or [`CallTarget::Dynamic`].
        target: CallTarget,
    },
    /// `GO TO name`.
    GoTo {
        /// The paragraph control passes to.
        target: String,
    },
    /// `OPEN` with one entry per file and its mode.
    Open {
        /// The files opened, each with its read and write mode.
        files: Vec<FileUse>,
    },
    /// `READ file [INTO field]`.
    Read {
        /// The file read.
        file: String,
        /// The INTO field, when one is named.
        into_field: Option<String>,
    },
    /// `WRITE record [FROM field]`.
    Write {
        /// The record written.
        record: String,
        /// The FROM field, when one is named.
        from_field: Option<String>,
    },
    /// `REWRITE record`.
    Rewrite {
        /// The record rewritten.
        record: String,
    },
    /// `DELETE file`.
    Delete {
        /// The file deleted from.
        file: String,
    },
    /// `CLOSE file ...`.
    Close {
        /// The files closed.
        files: Vec<String>,
    },
    /// `SELECT file ASSIGN TO name`.
    Select {
        /// The file selected.
        file: String,
        /// The ASSIGN name.
        assign: String,
    },
    /// An `FD` record entry.
    FileDesc {
        /// The record name.
        record: String,
    },
    /// `EXEC SQL` with one of the four table operations.
    ExecSql {
        /// SELECT, INSERT, UPDATE or DELETE.
        operation: String,
        /// The table names the statement reads or writes.
        tables: Vec<String>,
    },
    /// `EXEC CICS` with its operation and program.
    ExecCics {
        /// LINK, XCTL, or the operation named.
        operation: String,
        /// The PROGRAM value, when a literal names one.
        program: Option<String>,
    },
    /// `MOVE ... TO target ...`.
    Move {
        /// The target fields.
        targets: Vec<String>,
    },
    /// `COMPUTE target = ...`.
    Compute {
        /// The target fields.
        targets: Vec<String>,
    },
    /// `ADD ... TO | GIVING target`.
    Add {
        /// The target fields.
        targets: Vec<String>,
    },
    /// `SUBTRACT ... FROM | GIVING target`.
    Subtract {
        /// The target fields.
        targets: Vec<String>,
    },
    /// A `COPY` statement: expansion happens through the resolver, and this
    /// node marks where it happened.
    Copy {
        /// The copybook name.
        name: String,
        /// Whether the resolver supplied a body expanded after this node.
        resolved: bool,
    },
    /// Any other line: kept with its text and lines, claimed to mean nothing.
    Other {
        /// The line text.
        text: String,
    },
}

/// What a CALL transfers control to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum CallTarget {
    /// A quoted program literal.
    Literal(String),
    /// A variable target: a dynamic call whose target is unknown.
    Dynamic,
}

/// One file an OPEN statement names, with the mode it opens it in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct FileUse {
    /// The file name, folded to uppercase.
    pub(crate) name: String,
    /// Whether the mode reads: INPUT or I-O, or no mode at all.
    pub(crate) read: bool,
    /// Whether the mode writes: OUTPUT, EXTEND or I-O, or no mode at all.
    pub(crate) write: bool,
}

/// One COPY the source names: resolved through the caller's resolver, or
/// recorded as a gap the summary cannot tell about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct CopyRef {
    /// The copybook name.
    pub(crate) name: String,
    /// The one-based line of the COPY statement.
    pub(crate) line: usize,
    /// Whether the resolver supplied a body that was expanded here.
    pub(crate) resolved: bool,
}

impl StatementKind {
    /// The statement kind the verdict prints: `perform`, `call`, `go`,
    /// `open`, `read`, `write`, `rewrite`, `delete`, `close`, `select`,
    /// `fd`, `exec-sql`, `exec-cics`, `move`, `compute`, `add`,
    /// `subtract`, `copy` or `other`.
    pub(crate) const fn kind_name(&self) -> &'static str {
        match *self {
            Self::Perform { .. } => "perform",
            Self::Call { .. } => "call",
            Self::GoTo { .. } => "go",
            Self::Open { .. } => "open",
            Self::Read { .. } => "read",
            Self::Write { .. } => "write",
            Self::Rewrite { .. } => "rewrite",
            Self::Delete { .. } => "delete",
            Self::Close { .. } => "close",
            Self::Select { .. } => "select",
            Self::FileDesc { .. } => "fd",
            Self::ExecSql { .. } => "exec-sql",
            Self::ExecCics { .. } => "exec-cics",
            Self::Move { .. } => "move",
            Self::Compute { .. } => "compute",
            Self::Add { .. } => "add",
            Self::Subtract { .. } => "subtract",
            Self::Copy { .. } => "copy",
            Self::Other { .. } => "other",
        }
    }
}

/// Reads `source` into one program: divisions, paragraphs and statements.
///
/// `copy` resolves a COPY name to its body, or `None` when no entry names
/// it: an unresolved copy is recorded, never invented. No file input or
/// output happens here; the caller owns the filesystem behind `copy`.
///
/// # Errors
///
/// A [`LegacyError`] past any ceiling, or the first refusal the source earns.
pub fn parse_program(
    source: &str,
    limits: &LegacyLimits,
    copy: &dyn Fn(&str) -> Option<String>,
) -> Result<CobolProgram, LegacyError> {
    limits.check_bytes(source.len())?;
    let (dialect, unknown_directives) = detect_dialect(source, limits)?;
    let own = to_logical_lines(source, limits, 0)?;
    let statements = read_statements(&own, limits, copy, 0)?;
    assemble(statements, &unknown_directives, dialect, limits)
}

/// Assembles the program: the PROGRAM-ID, the paragraph table, and the COPY record.
fn assemble(
    statements: Vec<Statement>,
    unknown_directives: &[UnknownDirective],
    dialect: Dialect,
    limits: &LegacyLimits,
) -> Result<CobolProgram, LegacyError> {
    let mut name = String::from("UNNAMED");
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut copies: Vec<CopyRef> = Vec::new();
    let mut division = String::new();
    let mut expecting_name = false;
    for statement in statements.iter() {
        limits.check_nodes(paragraphs.len().saturating_add(statements.len()))?;
        let upper = uppercased(statement.text.as_str());
        let words: Vec<&str> = upper.split_whitespace().collect();
        if is_division(words.as_slice()) {
            if let &[first, ..] = words.as_slice() {
                division = String::from(first);
            }
            expecting_name = false;
            continue;
        }
        if expecting_name {
            expecting_name = false;
            if let &[only] = words.as_slice() {
                name = String::from(stripped(only));
                continue;
            }
        }
        if division == "PROCEDURE"
            && let Some(paragraph) = read_paragraph(words.as_slice(), statement)
        {
            paragraphs.push(paragraph);
            continue;
        }
        if words
            .first()
            .is_some_and(|word| stripped(word) == "PROGRAM-ID")
        {
            if let Some(found) = words.get(1) {
                name = String::from(stripped(found));
            } else {
                expecting_name = true;
            }
            continue;
        }
        if let StatementKind::Copy {
            ref name,
            ref resolved,
        } = statement.kind
        {
            copies.push(CopyRef {
                name: name.clone(),
                line: statement.lines.start(),
                resolved: *resolved,
            });
        }
    }
    Ok(CobolProgram {
        name,
        paragraphs,
        statements,
        copies,
        unknown_directives: unknown_directives.to_vec(),
        dialect,
    })
}

/// Whether the words open a new division.
fn is_division(words: &[&str]) -> bool {
    let &[first, second, ..] = words else {
        return false;
    };
    if stripped(second) != "DIVISION" {
        return false;
    }
    DIVISION_WORDS.contains(&first)
}

/// Reads a paragraph or section declaration from the words, or `None`.
///
/// A declaration is one bare name, or a name with SECTION, and never a word
/// that opens a statement.
fn read_paragraph(words: &[&str], statement: &Statement) -> Option<Paragraph> {
    if let &[first, second] = words
        && stripped(second) == "SECTION"
    {
        let name = String::from(stripped(first));
        if STATEMENT_WORDS.contains(&name.as_str()) {
            return None;
        }
        return Some(Paragraph {
            name: name.clone(),
            section: Some(name),
            lines: statement.lines,
        });
    }
    if let &[only] = words {
        let bare = stripped(only);
        if STATEMENT_WORDS.contains(&bare) {
            return None;
        }
        if bare
            .chars()
            .all(|cell| cell.is_ascii_alphanumeric() || cell == '-')
        {
            return Some(Paragraph {
                name: String::from(bare),
                section: None,
                lines: statement.lines,
            });
        }
        return None;
    }
    None
}
