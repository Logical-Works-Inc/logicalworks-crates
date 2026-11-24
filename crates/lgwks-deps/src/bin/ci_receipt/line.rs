//! One log line, read down to its text.
//!
//! A line of an Actions log arrives in one of two forms: the raw API form
//! `2026-11-24T15:22:46.9285151Z <text>`, or the `gh run view --log` form
//! `<job>\t<step>\t2026-11-24T15:22:46.9285151Z <text>`. Either may open with a
//! byte order mark and either may carry colour codes, spelled with the real
//! escape byte or, once a tool has printed it, as the two characters `^[`.
//! Everything here is text in, text out, with a bound on what comes out.

/// The most characters one cleaned line keeps. A line longer than any rule reads
/// is a payload, not a message, and every rule's window sits inside it.
pub(crate) const MAX_LINE_CHARS: usize = 4_096;

/// The shape of a timestamp's date and time, `d` standing for a digit.
const STAMP: &str = "dddd-dd-ddTdd:dd:dd";

/// The text of `raw`: the job and step columns, the timestamp, the colour codes
/// and every control character but the tab gone, cut to [`MAX_LINE_CHARS`].
pub(crate) fn clean(raw: &str) -> String {
    let unmarked = if let Some(rest) = raw.strip_prefix('\u{feff}') {
        rest
    } else {
        raw
    };
    strip_colour(after_stamp(unmarked))
        .chars()
        .filter(|found| !found.is_control() || *found == '\t')
        .take(MAX_LINE_CHARS)
        .collect()
}

/// `text` cut to `limit` characters, on a character boundary.
pub(crate) fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `text` as a number, when it is only digits and fits a `u32`.
pub(crate) fn number(text: &str) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    text.chars().try_fold(0_u32, |total, digit| {
        total.checked_mul(10)?.checked_add(digit.to_digit(10)?)
    })
}

/// The part of `line` after its timestamp, in either form; the whole line when
/// it carries none.
fn after_stamp(line: &str) -> &str {
    if let Some(rest) = stamped(line) {
        return rest;
    }
    let mut columns = line.splitn(3, '\t');
    if let (Some(_), Some(_), Some(tail)) = (columns.next(), columns.next(), columns.next())
        && let Some(rest) = stamped(tail)
    {
        return rest;
    }
    line
}

/// The text after a leading `YYYY-MM-DDTHH:MM:SS[.fraction]Z` and the one space
/// that follows it; `None` when `text` does not open with one.
fn stamped(text: &str) -> Option<&str> {
    let mut chars = text.chars();
    for shape in STAMP.chars() {
        let found = chars.next()?;
        let fits = if shape == 'd' {
            found.is_ascii_digit()
        } else {
            found == shape
        };
        if !fits {
            return None;
        }
    }
    let rest = text.get(STAMP.len()..)?;
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => fraction.trim_start_matches(|digit: char| digit.is_ascii_digit()),
        None => rest,
    };
    let after = rest.strip_prefix('Z')?;
    match after.strip_prefix(' ') {
        Some(text_after) => Some(text_after),
        None if after.is_empty() => Some(after),
        None => None,
    }
}

/// `text` without its colour codes, real or spelled `^[`.
fn strip_colour(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['\u{1b}', '^']) {
        let Some((before, from)) = rest.split_at_checked(at) else {
            break;
        };
        kept.push_str(before);
        let dropped = escape_len(from);
        let skip = if dropped == 0 {
            kept.push('^');
            1
        } else {
            dropped
        };
        let Some((_, after)) = from.split_at_checked(skip) else {
            break;
        };
        rest = after;
    }
    kept.push_str(rest);
    kept
}

/// The bytes of the colour code that opens `from`, which opens with the escape
/// byte or with `^`: 0 when a `^` opens none, and 1 for an escape byte that opens
/// no sequence, which is dropped alone.
fn escape_len(from: &str) -> usize {
    let (lead, body, bare) = if let Some(body) = from.strip_prefix('\u{1b}') {
        (1_usize, body, 1_usize)
    } else if let Some(body) = from.strip_prefix("^[") {
        (2, body, 0)
    } else {
        return 0;
    };
    let Some(params) = body.strip_prefix('[') else {
        return bare;
    };
    let mut length = 0_usize;
    for found in params.chars() {
        length = length.saturating_add(found.len_utf8());
        if found.is_ascii_alphabetic() || found == '~' {
            return lead.saturating_add(1).saturating_add(length);
        }
        if !(found.is_ascii_digit() || matches!(found, ';' | ':' | '?')) {
            break;
        }
    }
    bare
}
