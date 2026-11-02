//! The JCL parameters: splitting, substitution and DISP direction.
//!
//! Parameters split at top-level commas: commas inside quotes or parentheses
//! never split. `&NAME` references substitute from the symbol table, with one
//! trailing period terminating the reference; `&&NAME` is a temporary dataset
//! name and is kept as written. DISP decides the direction (z/OS MVS JCL
//! Reference, DD DISP): SHR reads, OLD is exclusive use of an existing dataset
//! so it may read and write, NEW and MOD (append) write, and a missing DISP is
//! NEW. `DD *` and `DD DATA` are in-stream input the step reads; SYSOUT is
//! output the step writes.

use super::super::uppercased;
use super::{Dataset, Symbol, step_name};

/// Reads one DD card into its dataset, substituting symbolics.
pub(crate) fn read_dataset(
    card: &super::cards::Card,
    symbols: &[Symbol],
) -> Result<(Dataset, Vec<String>), super::super::error::LegacyError> {
    let pairs = split_params(card.params.as_str());
    let mut dsn: Option<String> = None;
    let mut disp = String::new();
    let mut stream = Stream::Dataset;
    for pair in pairs.iter() {
        let (key, value) = split_pair(pair);
        if key == "DSN" || key == "DSNAME" {
            dsn = Some(String::from(value));
        } else if key == "DISP" {
            disp = String::from(value);
        } else if key == "SYSOUT" {
            stream = Stream::Sysout;
        } else if *pair == "*" || *pair == "DATA" {
            stream = Stream::Instream;
        }
    }
    let mut unresolved: Vec<String> = Vec::new();
    if let Some(name) = dsn.clone() {
        let (text, missing) = substitute(name.as_str(), symbols);
        dsn = Some(text);
        unresolved.extend(missing);
    }
    if stream != Stream::Dataset {
        dsn = None;
    }
    let status = disp_status(disp.as_str());
    let (reads, writes) = disp_direction(status.as_str(), stream);
    let range = match super::card_range(card) {
        Some(range) => range,
        None => {
            let refusal = Err(super::super::error::LegacyError::UnknownStatement {
                line: card.start,
                text: String::from(card.params.as_str()),
            });
            tracing::debug!("jcl: refusing a card past the line numbering");
            return refusal;
        }
    };
    Ok((
        Dataset {
            name: step_name(card),
            dsn,
            disp: status,
            reads,
            writes,
            lines: range,
        },
        unresolved,
    ))
}

/// The DISP status: the first element, or NEW when none is written.
pub(crate) fn disp_status(disp: &str) -> String {
    let bare = disp.trim().trim_start_matches('(').trim_end_matches(')');
    let first = match bare.split(',').next() {
        Some(first) => first.trim(),
        None => "",
    };
    if first.is_empty() {
        return String::from("NEW");
    }
    uppercased(first)
}

/// What a DD card points at: a cataloged dataset, in-stream input, or SYSOUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stream {
    /// A dataset named by DSN, or a temporary one, directed by DISP.
    Dataset,
    /// `DD *` or `DD DATA`: in-stream input the step reads.
    Instream,
    /// `SYSOUT=`: output the step writes.
    Sysout,
}

/// The read and write arms a DD card earns from its stream and DISP status.
pub(crate) fn disp_direction(status: &str, stream: Stream) -> (bool, bool) {
    match stream {
        Stream::Instream => return (true, false),
        Stream::Sysout => return (false, true),
        Stream::Dataset => {}
    }
    match status {
        "SHR" => (true, false),
        "OLD" => (true, true),
        _ => (false, true),
    }
}

/// The INCLUDE member name: `MEMBER=name` or the bare word.
pub(crate) fn include_member(params: &str) -> String {
    for pair in split_params(params).iter() {
        let (key, value) = split_pair(pair);
        if key == "MEMBER" && !value.is_empty() {
            return uppercased(value);
        }
    }
    uppercased(params.trim())
}

/// Splits parameters at top-level commas: commas inside quotes or
/// parentheses never split.
pub(crate) fn split_params(params: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = Vec::new();
    let mut quoted = false;
    let mut depth = 0_usize;
    let mut start = 0_usize;
    for (offset, cell) in params.char_indices() {
        if cell == '\'' {
            quoted = !quoted;
        }
        if !quoted {
            if cell == '(' {
                depth = depth.saturating_add(1);
            } else if cell == ')' && depth > 0 {
                depth = depth.saturating_sub(1);
            }
        }
        if cell == ',' && !quoted && depth == 0 {
            if let Some(part) = params.get(start..offset) {
                parts.push(part.trim());
            }
            start = offset.saturating_add(1);
        }
    }
    if let Some(rest) = params.get(start..)
        && (!rest.trim().is_empty() || parts.is_empty())
    {
        parts.push(rest.trim());
    }
    parts
}

/// Splits one `KEY=value` pair at its first `=`.
pub(crate) fn split_pair(pair: &str) -> (&str, &str) {
    match pair.find('=') {
        Some(at) => {
            let key = match pair.get(..at) {
                Some(key) => key.trim(),
                None => "",
            };
            let value = match pair.get(at.saturating_add(1)..) {
                Some(value) => value.trim().trim_matches('\''),
                None => "",
            };
            (key, value)
        }
        None => (pair.trim(), ""),
    }
}

/// Substitutes `&NAME` references from `symbols`: the text and the names no
/// entry supplied. One trailing period terminates the reference, so
/// `&HLQ..DATA` reads as the value with `.DATA` after it.
pub(crate) fn substitute(text: &str, symbols: &[Symbol]) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut missing: Vec<String> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(cell) = chars.next() {
        if cell != '&' {
            out.push(cell);
            continue;
        }
        if chars.peek() == Some(&'&') {
            // `&&NAME` names a temporary dataset, not a symbolic: kept as written.
            chars.next();
            out.push_str("&&");
            continue;
        }
        let mut name = String::new();
        while let Some(seen) = chars.peek() {
            if seen.is_ascii_alphanumeric() || *seen == '#' || *seen == '@' || *seen == '$' {
                name.push(*seen);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            out.push('&');
            continue;
        }
        let upper = uppercased(name.as_str());
        let mut found: Option<&str> = None;
        for entry in symbols.iter() {
            if entry.name == upper {
                found = Some(entry.value.as_str());
                break;
            }
        }
        match found {
            Some(value) => out.push_str(value),
            None => {
                if !missing.contains(&upper) {
                    missing.push(upper.clone());
                }
                out.push('&');
                out.push_str(upper.as_str());
            }
        }
        if chars.peek() == Some(&'.') {
            chars.next();
        }
    }
    (out, missing)
}
