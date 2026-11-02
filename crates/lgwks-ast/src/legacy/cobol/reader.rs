//! The COBOL line reader: format handling, statement assembly and COPY.
//!
//! Fixed and free format become [`LogicalLine`]s, periods split them into
//! statements, EXEC blocks run to their END-EXEC, and COPY nodes expand
//! through the caller's resolver with the COPY statement's own attribution.

use super::super::{LegacyError, LegacyLimits, LineRange, uppercased};
use super::classify::classify;
use super::{Statement, StatementKind};

/// One physical code line with the program line it is attributed to.
///
/// Lines of the program itself attribute to their own physical number.
/// Lines expanded from a COPY attribute to the COPY statement's own line:
/// the source line that brought them, which the program text holds.
pub(crate) struct LogicalLine {
    /// The code text, without sequence numbers or comments.
    text: String,
    /// The one-based program line this line is attributed to.
    attr_line: usize,
}

/// One pending statement under assembly: its words so far and its span.
struct Draft {
    /// The accumulated text, lines joined with one blank.
    text: String,
    /// The attributed first line.
    start: usize,
    /// The attributed last line so far.
    end: usize,
}

/// Reads `source` into code lines: fixed format until a free-format
/// directive, free format after it, comments and sequence numbers dropped.
///
/// `base` shifts physical numbers for bodies read from elsewhere; the
/// program's own read passes zero.
pub(crate) fn to_logical_lines(
    source: &str,
    limits: &LegacyLimits,
    base: usize,
) -> Result<Vec<LogicalLine>, LegacyError> {
    let mut lines: Vec<LogicalLine> = Vec::new();
    let mut free = false;
    for (offset, raw) in source.lines().enumerate() {
        let line_no = offset.saturating_add(1).saturating_add(base);
        limits.check_lines(lines.len().saturating_add(1))?;
        if !free && is_free_directive(raw) {
            free = true;
        }
        match code_text(raw, free) {
            Some(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                lines.push(LogicalLine {
                    text,
                    attr_line: line_no,
                });
            }
            None => continue,
        }
    }
    Ok(lines)
}

/// Whether `raw` switches the reader to free format.
fn is_free_directive(raw: &str) -> bool {
    let upper = uppercased(raw);
    let trimmed = upper.trim_start();
    let code = fixed_code(trimmed);
    let area = match code {
        Some(area) => area,
        None => trimmed,
    };
    let area = area.trim_start();
    area.starts_with(">>SOURCE FORMAT IS FREE")
        || area.starts_with(">>SOURCE FORMAT FREE")
        || area.starts_with(">>SOURCE FORMAT IS FREE.")
        || area.starts_with(">>SOURCE FORMAT FREE.")
}

/// The code area of a fixed-format line, or `None` for a comment line.
///
/// Sequence columns and columns past 72 are dropped; the indicator decides.
fn fixed_code(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    if bytes.len() < 8 {
        let trimmed = line.trim();
        if trimmed.starts_with('*') || trimmed.starts_with('/') {
            return None;
        }
        return Some(line);
    }
    let indicator = match bytes.get(6) {
        Some(cell) => *cell,
        None => b' ',
    };
    if indicator == b'*' || indicator == b'/' {
        return None;
    }
    let end = bytes.len().min(72);
    match line.get(7..end) {
        Some(area) => Some(area),
        None => Some(line),
    }
}

/// The code text of `raw`, or `None` for a comment line.
fn code_text(raw: &str, free: bool) -> Option<String> {
    if free {
        let trimmed = raw.trim();
        if trimmed.starts_with('*') || trimmed.starts_with("*>") {
            return None;
        }
        return Some(String::from(trimmed));
    }
    let area = fixed_code(raw)?;
    let clipped = match area.find("*>") {
        Some(mark) => match area.get(..mark) {
            Some(kept) => kept,
            None => area,
        },
        None => area,
    };
    Some(String::from(clipped.trim_end()))
}

/// Splits `line` at statement ends: a period followed by blank space or the
/// line end stays with the statement it closes.
fn split_terminators(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0_usize;
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let closes = match bytes.get(offset) {
            Some(cell) => *cell == b'.',
            None => false,
        };
        let followed = match bytes.get(offset.saturating_add(1)) {
            Some(cell) => *cell == b' ' || *cell == b'\t',
            None => true,
        };
        if closes && followed {
            let end = offset.saturating_add(1);
            if let Some(part) = line.get(start..end) {
                parts.push(part);
            }
            start = end;
        }
        offset = offset.saturating_add(1);
    }
    if let Some(rest) = line.get(start..)
        && (!rest.trim().is_empty() || parts.is_empty())
    {
        parts.push(rest);
    }
    parts
}

/// Reads statements from `lines`, expanding COPY through `copy` at `depth`.
///
/// Statements from an expanded body attribute to the COPY statement's own
/// line. `depth` counts the COPY nesting already open.
pub(crate) fn read_statements(
    lines: &[LogicalLine],
    limits: &LegacyLimits,
    copy: &dyn Fn(&str) -> Option<String>,
    depth: usize,
) -> Result<Vec<Statement>, LegacyError> {
    let mut statements: Vec<Statement> = Vec::new();
    let mut pending = Draft {
        text: String::new(),
        start: 0,
        end: 0,
    };
    let mut in_exec = false;
    for line in lines.iter() {
        for part in split_terminators(line.text.as_str()) {
            if part.trim().is_empty() {
                continue;
            }
            if pending.text.is_empty() {
                pending.start = line.attr_line;
            }
            if !pending.text.is_empty() {
                pending.text.push(' ');
            }
            pending.text.push_str(part.trim());
            pending.end = line.attr_line;
            let upper = uppercased(pending.text.as_str());
            if starts_exec(upper.as_str()) {
                in_exec = true;
            }
            let complete = part.trim_end().ends_with('.');
            if in_exec {
                if upper.contains("END-EXEC") {
                    in_exec = false;
                    emit_statement(&mut statements, &mut pending, limits)?;
                }
                continue;
            }
            if complete {
                emit_statement(&mut statements, &mut pending, limits)?;
            }
        }
    }
    if !pending.text.trim().is_empty() {
        emit_statement(&mut statements, &mut pending, limits)?;
    }
    expand_copies(&mut statements, limits, copy, depth)?;
    Ok(statements)
}

/// Whether the accumulated statement opens an EXEC block.
fn starts_exec(upper: &str) -> bool {
    match upper.split_whitespace().next() {
        Some(word) => word == "EXEC",
        None => false,
    }
}

/// Classifies the pending statement, appends its node, and clears it.
fn emit_statement(
    statements: &mut Vec<Statement>,
    pending: &mut Draft,
    limits: &LegacyLimits,
) -> Result<(), LegacyError> {
    limits.check_nodes(statements.len().saturating_add(1))?;
    let upper = uppercased(pending.text.as_str());
    let words: Vec<&str> = upper.split_whitespace().collect();
    let kind = classify(words.as_slice(), pending.text.as_str());
    let start = match LineRange::new(pending.start, pending.end) {
        Ok(range) => range,
        Err(error) => {
            let refusal = Err(error);
            tracing::debug!("cobol: the pending span left the line numbering");
            return refusal;
        }
    };
    statements.push(Statement {
        kind,
        lines: start,
        text: String::from(pending.text.trim()),
    });
    pending.text.clear();
    Ok(())
}

/// Expands every COPY node in `statements` through `copy`, keeping the COPY
/// node marked resolved and splicing the body's statements after it with
/// its line attribution. A COPY no resolver entry names, or whose
/// REPLACING clause holds any other form, stays standing unresolved.
fn expand_copies(
    statements: &mut Vec<Statement>,
    limits: &LegacyLimits,
    copy: &dyn Fn(&str) -> Option<String>,
    depth: usize,
) -> Result<(), LegacyError> {
    let mut expanded: Vec<Statement> = Vec::new();
    let mut copies: usize = 0;
    for statement in statements.iter() {
        let wanted = match statement.kind {
            StatementKind::Copy { ref name, .. } => Some(name.clone()),
            _ => None,
        };
        let Some(wanted) = wanted else {
            expanded.push(statement.clone());
            continue;
        };
        copies = copies.saturating_add(1);
        limits.check_copies(copies)?;
        limits.check_copy_depth(depth.saturating_add(1))?;
        let body = match copy(wanted.as_str()) {
            Some(body) => body,
            None => {
                expanded.push(statement.clone());
                continue;
            }
        };
        let shaped = apply_replacing(body.as_str(), statement.text.as_str());
        let Some(text) = shaped else {
            expanded.push(statement.clone());
            continue;
        };
        expanded.push(Statement {
            kind: StatementKind::Copy {
                name: wanted.clone(),
                resolved: true,
            },
            lines: statement.lines,
            text: String::from(statement.text.as_str()),
        });
        let mut lines = to_logical_lines(text.as_str(), limits, 0)?;
        for line in lines.iter_mut() {
            line.attr_line = statement.lines.start();
        }
        let mut inner = read_statements(lines.as_slice(), limits, copy, depth.saturating_add(1))?;
        expanded.append(&mut inner);
    }
    *statements = expanded;
    Ok(())
}

/// Applies a COPY's `==` to `==` REPLACING pairs to `body`.
///
/// The pairs are read from the COPY `statement`: each `==from== BY ==to==`
/// rewrites the bare `from` text-word in the body to `to`. Matching is by
/// text-words, so `:PFX:-ID` matches `:PFX:` (the `-` bounds the word) while
/// `XTAG` never matches `TAG` (a letter bounds it). `BY` is found
/// case-insensitively. Returns `None` when the REPLACING clause holds any
/// other form: the copy stays unresolved rather than half-applied.
fn apply_replacing(body: &str, statement: &str) -> Option<String> {
    let upper = uppercased(statement);
    let at = match upper.find("REPLACING") {
        Some(at) => at,
        None => return Some(String::from(body)),
    };
    let marker = match statement.get(at..at.saturating_add(9)) {
        Some(marker) if marker.eq_ignore_ascii_case("replacing") => marker.len(),
        Some(_) | None => return None,
    };
    let clause = statement.get(at.saturating_add(marker)..)?;
    let mut out = String::from(body);
    let mut rest = clause;
    let mut pairs: usize = 0;
    while let Some(first) = rest.find("==") {
        let after_first = rest.get(first.saturating_add(2)..)?;
        let second = after_first.find("==")?;
        let from = after_first.get(..second)?;
        let after_second = after_first.get(second.saturating_add(2)..)?;
        let upper_second = uppercased(after_second);
        let by = upper_second.find("BY")?;
        let after_by = after_second.get(by.saturating_add(2)..)?;
        let third = after_by.find("==")?;
        let after_third = after_by.get(third.saturating_add(2)..)?;
        let fourth = after_third.find("==")?;
        let to = after_third.get(..fourth)?;
        if from.is_empty() {
            return None;
        }
        out = replace_word(out.as_str(), from, to);
        pairs = pairs.saturating_add(1);
        rest = "";
        if let Some(found) = after_third.get(fourth.saturating_add(2)..) {
            rest = found;
        }
    }
    if pairs == 0 {
        return None;
    }
    let leftover = rest.trim().trim_end_matches('.').trim();
    if !leftover.is_empty() {
        return None;
    }
    Some(out)
}

/// Replaces every text-word occurrence of `from` in `haystack` with `to`.
///
/// A match counts only when both sides bound it by a non-letter-or-digit
/// (or the string end), so `:PFX:-ID` matches `:PFX:` while `XTAG` never
/// matches `TAG`. The search is case-sensitive: COBOL folds the program
/// before comparing, and the body here is already folded by the reader.
fn replace_word(haystack: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return String::from(haystack);
    }
    let mut out = String::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(at) = rest.find(from) {
        let before_ok = match rest.get(..at) {
            Some(before) => match before.chars().next_back() {
                Some(cell) => !cell.is_ascii_alphanumeric(),
                None => true,
            },
            None => true,
        };
        let after_start = at.saturating_add(from.len());
        let after_ok = match rest.get(after_start..) {
            Some(after) => match after.chars().next() {
                Some(cell) => !cell.is_ascii_alphanumeric(),
                None => true,
            },
            None => true,
        };
        // A match is copied up to its end and replaced; a non-match keeps its
        // first character and the search resumes one whole character later,
        // so a multibyte character is never split.
        let resume = if before_ok && after_ok {
            after_start
        } else {
            match rest.get(at..).and_then(|tail| tail.chars().next()) {
                Some(cell) => at.saturating_add(cell.len_utf8()),
                None => break,
            }
        };
        let (Some(head), Some(tail)) = (rest.get(..resume), rest.get(resume..)) else {
            break;
        };
        if before_ok && after_ok {
            if let Some(before) = head.get(..at) {
                out.push_str(before);
            }
            out.push_str(to);
        } else {
            out.push_str(head);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}
