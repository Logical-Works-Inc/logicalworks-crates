//! The COBOL statement classifiers: one recorded kind per statement (#777).
//!
//! Classification reads the statement's words only, never the program around
//! it. Anything outside the recorded set is [`StatementKind::Other`]: kept
//! with its text and lines, and never claimed to mean anything.

use super::super::{is_count, stripped, unquoted};
use super::{CallTarget, FileUse, StatementKind};

/// Statement targets the MOVE family reads past: the words between the verb
/// and the targets name the sources, never a target.
const MOVE_SKIPS: [&str; 3] = ["CORRESPONDING", "CORR", "TO"];

/// Classifies one statement's words: the recorded kind, or Other.
pub(crate) fn classify(words: &[&str], text: &str) -> StatementKind {
    let first = match words.first() {
        Some(word) => *word,
        None => {
            return StatementKind::Other {
                text: String::from(text),
            };
        }
    };
    match first {
        "PERFORM" => classify_perform(words),
        "CALL" => classify_call(words),
        "GO" | "GOTO" => classify_goto(words),
        "OPEN" => classify_open(words),
        "READ" => classify_read(words),
        "WRITE" => classify_write(words),
        "REWRITE" => classify_rewrite(words),
        "DELETE" => classify_delete(words),
        "CLOSE" => classify_close(words),
        "SELECT" => classify_select(words),
        "FD" => classify_fd(words),
        "EXEC" => classify_exec(words),
        "MOVE" => classify_move(words),
        "COMPUTE" => classify_compute(words),
        "ADD" => classify_add(words),
        "SUBTRACT" => classify_subtract(words),
        "COPY" => classify_copy(words),
        _ => StatementKind::Other {
            text: String::from(text),
        },
    }
}

/// Classifies a PERFORM: its target, its THRU end, and whether a condition
/// or an inline body drives it.
fn classify_perform(words: &[&str]) -> StatementKind {
    let mut target: Option<String> = None;
    let mut thru: Option<String> = None;
    let mut conditional = false;
    let mut offset = 1_usize;
    if let Some(word) = words.get(offset) {
        let bare = stripped(word);
        if bare == "UNTIL"
            || bare == "VARYING"
            || bare == "TIMES"
            || bare == "WITH"
            || is_count(bare)
        {
            conditional = true;
        } else {
            target = Some(String::from(bare));
            offset = offset.saturating_add(1);
        }
    }
    while let Some(word) = words.get(offset) {
        let bare = stripped(word);
        if (bare == "THRU" || bare == "THROUGH")
            && let Some(end) = words.get(offset.saturating_add(1))
        {
            thru = Some(String::from(stripped(end)));
            offset = offset.saturating_add(2);
            continue;
        }
        if bare == "UNTIL" || bare == "VARYING" || bare == "TIMES" || bare == "TEST" {
            conditional = true;
        }
        offset = offset.saturating_add(1);
    }
    StatementKind::Perform {
        target,
        thru,
        conditional,
    }
}

/// Classifies a CALL: a quoted literal, or a dynamic call with no target.
fn classify_call(words: &[&str]) -> StatementKind {
    let raw = match words.get(1) {
        Some(word) => *word,
        None => {
            return StatementKind::Call {
                target: CallTarget::Dynamic,
            };
        }
    };
    let bare = stripped(raw);
    if bare.starts_with('\'') || bare.starts_with('"') {
        let literal = unquoted(bare);
        return StatementKind::Call {
            target: CallTarget::Literal(String::from(literal)),
        };
    }
    StatementKind::Call {
        target: CallTarget::Dynamic,
    }
}

/// Classifies a GO TO: the paragraph control passes to.
fn classify_goto(words: &[&str]) -> StatementKind {
    let mut target = "UNKNOWN";
    for word in words.iter() {
        let bare = stripped(word);
        if bare != "GO" && bare != "TO" && bare != "GOTO" && !bare.is_empty() {
            target = bare;
        }
    }
    if target == "UNKNOWN" {
        return StatementKind::Other {
            text: words.join(" "),
        };
    }
    StatementKind::GoTo {
        target: String::from(target),
    }
}

/// Classifies an OPEN: one entry per file with the mode's read and write arms.
fn classify_open(words: &[&str]) -> StatementKind {
    let mut reading = false;
    let mut writing = false;
    let mut seen_mode = false;
    let mut files: Vec<FileUse> = Vec::new();
    for word in words.iter().skip(1) {
        let bare = stripped(word);
        match bare {
            "INPUT" => {
                reading = true;
                seen_mode = true;
            }
            "OUTPUT" | "EXTEND" => {
                writing = true;
                seen_mode = true;
            }
            "I-O" => {
                reading = true;
                writing = true;
                seen_mode = true;
            }
            "I" | "O" => {}
            _ => {
                if bare.is_empty() {
                    continue;
                }
                files.push(FileUse {
                    name: String::from(bare),
                    read: reading,
                    write: writing,
                });
                reading = false;
                writing = false;
            }
        }
    }
    if files.is_empty() {
        return StatementKind::Other {
            text: words.join(" "),
        };
    }
    if !seen_mode {
        for entry in files.iter_mut() {
            entry.read = false;
            entry.write = false;
        }
    }
    StatementKind::Open { files }
}

/// Classifies a READ: its file and its INTO field.
fn classify_read(words: &[&str]) -> StatementKind {
    let file = match words.get(1) {
        Some(word) => String::from(stripped(word)),
        None => {
            return StatementKind::Other {
                text: words.join(" "),
            };
        }
    };
    let mut into_field: Option<String> = None;
    let mut offset = 2_usize;
    while let Some(word) = words.get(offset) {
        if *word == "INTO" {
            if let Some(next) = words.get(offset.saturating_add(1)) {
                into_field = Some(String::from(stripped(next)));
            }
            break;
        }
        offset = offset.saturating_add(1);
    }
    StatementKind::Read { file, into_field }
}

/// Classifies a WRITE: its record and its FROM field.
fn classify_write(words: &[&str]) -> StatementKind {
    let record = match words.get(1) {
        Some(word) => String::from(stripped(word)),
        None => {
            return StatementKind::Other {
                text: words.join(" "),
            };
        }
    };
    let mut from_field: Option<String> = None;
    let mut offset = 2_usize;
    while let Some(word) = words.get(offset) {
        if *word == "FROM" {
            if let Some(next) = words.get(offset.saturating_add(1)) {
                from_field = Some(String::from(stripped(next)));
            }
            break;
        }
        offset = offset.saturating_add(1);
    }
    StatementKind::Write { record, from_field }
}

/// Classifies a REWRITE: the record it rewrites.
fn classify_rewrite(words: &[&str]) -> StatementKind {
    match words.get(1) {
        Some(word) => StatementKind::Rewrite {
            record: String::from(stripped(word)),
        },
        None => StatementKind::Other {
            text: words.join(" "),
        },
    }
}

/// Classifies a DELETE: the file it deletes from.
fn classify_delete(words: &[&str]) -> StatementKind {
    match words.get(1) {
        Some(word) => StatementKind::Delete {
            file: String::from(stripped(word)),
        },
        None => StatementKind::Other {
            text: words.join(" "),
        },
    }
}

/// Classifies a CLOSE: every file it closes.
fn classify_close(words: &[&str]) -> StatementKind {
    let mut files: Vec<String> = Vec::new();
    for word in words.iter().skip(1) {
        let bare = stripped(word);
        if !bare.is_empty() {
            files.push(String::from(bare));
        }
    }
    if files.is_empty() {
        return StatementKind::Other {
            text: words.join(" "),
        };
    }
    StatementKind::Close { files }
}

/// Classifies a SELECT: its file and its ASSIGN name.
fn classify_select(words: &[&str]) -> StatementKind {
    let file = match words.get(1) {
        Some(word) => String::from(stripped(word)),
        None => {
            return StatementKind::Other {
                text: words.join(" "),
            };
        }
    };
    let mut assign: Option<String> = None;
    let mut offset = 2_usize;
    while let Some(word) = words.get(offset) {
        if *word == "ASSIGN" {
            let mut next = offset.saturating_add(1);
            if words.get(next) == Some(&"TO") {
                next = next.saturating_add(1);
            }
            if let Some(name) = words.get(next) {
                assign = Some(String::from(stripped(name)));
            }
            break;
        }
        offset = offset.saturating_add(1);
    }
    match assign {
        Some(assign) => StatementKind::Select { file, assign },
        None => StatementKind::Other {
            text: words.join(" "),
        },
    }
}

/// Classifies an FD entry: the record it describes.
fn classify_fd(words: &[&str]) -> StatementKind {
    match words.get(1) {
        Some(word) => StatementKind::FileDesc {
            record: String::from(stripped(word)),
        },
        None => StatementKind::Other {
            text: words.join(" "),
        },
    }
}

/// Classifies an EXEC block: SQL table operations and CICS LINK and XCTL.
fn classify_exec(words: &[&str]) -> StatementKind {
    if words.get(1) == Some(&"SQL") {
        return classify_exec_sql(words);
    }
    if words.get(1) == Some(&"CICS") {
        return classify_exec_cics(words);
    }
    StatementKind::Other {
        text: words.join(" "),
    }
}

/// Classifies an EXEC SQL block: the operation and its table names.
fn classify_exec_sql(words: &[&str]) -> StatementKind {
    let mut operation: Option<String> = None;
    for word in words.iter() {
        if *word == "SELECT" || *word == "INSERT" || *word == "UPDATE" || *word == "DELETE" {
            operation = Some(String::from(*word));
            break;
        }
    }
    let Some(operation) = operation else {
        return StatementKind::Other {
            text: words.join(" "),
        };
    };
    let mut tables: Vec<String> = Vec::new();
    let mut offset = 0_usize;
    while let Some(word) = words.get(offset) {
        if (*word == "FROM" || *word == "INTO" || *word == "UPDATE" || *word == "TABLE")
            && let Some(next) = words.get(offset.saturating_add(1))
        {
            let bare = stripped(next);
            if !bare.is_empty() && !tables.contains(&String::from(bare)) {
                tables.push(String::from(bare));
            }
        }
        offset = offset.saturating_add(1);
    }
    StatementKind::ExecSql { operation, tables }
}

/// Classifies an EXEC CICS block: LINK and XCTL with their program literal.
fn classify_exec_cics(words: &[&str]) -> StatementKind {
    let operation = match words.get(2) {
        Some(word) => String::from(stripped(word)),
        None => {
            return StatementKind::Other {
                text: words.join(" "),
            };
        }
    };
    let mut program: Option<String> = None;
    for word in words.iter() {
        if word.starts_with("PROGRAM") {
            let inner = match word.find('(').and_then(|open| {
                word.find(')').and_then(|close| {
                    if close > open {
                        word.get(open.saturating_add(1)..close)
                    } else {
                        None
                    }
                })
            }) {
                Some(inner) => inner,
                None => continue,
            };
            let bare = stripped(inner.trim());
            program = Some(String::from(unquoted(bare)));
            break;
        }
    }
    StatementKind::ExecCics { operation, program }
}

/// Classifies a MOVE: every target past the last TO.
fn classify_move(words: &[&str]) -> StatementKind {
    let mut last_to: Option<usize> = None;
    for (offset, word) in words.iter().enumerate() {
        if *word == "TO" {
            last_to = Some(offset);
        }
    }
    let Some(found) = last_to else {
        return StatementKind::Other {
            text: words.join(" "),
        };
    };
    let mut targets: Vec<String> = Vec::new();
    for word in words.iter().skip(found.saturating_add(1)) {
        let bare = stripped(word);
        if bare.is_empty() || MOVE_SKIPS.contains(&bare) {
            continue;
        }
        targets.push(String::from(bare));
    }
    if targets.is_empty() {
        return StatementKind::Other {
            text: words.join(" "),
        };
    }
    StatementKind::Move { targets }
}

/// Classifies a COMPUTE: the words before the `=` sign.
fn classify_compute(words: &[&str]) -> StatementKind {
    let mut targets: Vec<String> = Vec::new();
    for word in words.iter().skip(1) {
        if word.contains('=') {
            let mut left = "";
            if let Some(found) = word.split('=').next() {
                left = found;
            }
            let bare = stripped(left.trim());
            if !bare.is_empty() && bare != "COMPUTE" {
                targets.push(String::from(bare));
            }
            break;
        }
        let bare = stripped(word);
        if !bare.is_empty() {
            targets.push(String::from(bare));
        }
    }
    if targets.is_empty() {
        return StatementKind::Other {
            text: words.join(" "),
        };
    }
    StatementKind::Compute { targets }
}

/// Classifies an ADD: every target past TO or GIVING.
fn classify_add(words: &[&str]) -> StatementKind {
    targets_past(words, &["TO", "GIVING"], "ADD")
}

/// Classifies a SUBTRACT: every target past FROM or GIVING.
fn classify_subtract(words: &[&str]) -> StatementKind {
    targets_past(words, &["FROM", "GIVING"], "SUBTRACT")
}

/// Classifies an ADD or SUBTRACT: the targets past any of `markers`.
fn targets_past(words: &[&str], markers: &[&str], verb: &str) -> StatementKind {
    let mut last: Option<usize> = None;
    for (offset, word) in words.iter().enumerate() {
        if markers.contains(word) {
            last = Some(offset);
        }
    }
    let Some(found) = last else {
        return StatementKind::Other {
            text: words.join(" "),
        };
    };
    let mut targets: Vec<String> = Vec::new();
    for word in words.iter().skip(found.saturating_add(1)) {
        let bare = stripped(word);
        if bare.is_empty() || bare == verb {
            continue;
        }
        targets.push(String::from(bare));
    }
    if targets.is_empty() {
        return StatementKind::Other {
            text: words.join(" "),
        };
    }
    if verb == "ADD" {
        return StatementKind::Add { targets };
    }
    StatementKind::Subtract { targets }
}

/// Classifies a COPY: the copybook name it names, not yet resolved.
fn classify_copy(words: &[&str]) -> StatementKind {
    match words.get(1) {
        Some(word) => {
            let bare = stripped(word);
            if bare.is_empty() {
                return StatementKind::Other {
                    text: words.join(" "),
                };
            }
            StatementKind::Copy {
                name: String::from(bare),
                resolved: false,
            }
        }
        None => StatementKind::Other {
            text: words.join(" "),
        },
    }
}
