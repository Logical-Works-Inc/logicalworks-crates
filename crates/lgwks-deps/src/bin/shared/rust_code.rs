//! Telling Rust code from the comments and literals that can spell it.
//!
//! The std-first audit and the unwrap lane both have to ask whether a word is
//! code. This is the one lexer they ask: included by each binary through
//! `#[path]`, as the other files of this directory are.

/// Whether `ch` continues an identifier or word.
pub(crate) fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The index of the first `needle` in `chars` at or after `from`.
fn find_sequence(chars: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    let last = chars.len().checked_sub(needle.len())?;
    (from..=last).find(|&at| chars.get(at..at.saturating_add(needle.len())) == Some(needle))
}

/// Blanks `start..end` of `out` to spaces, keeping newlines.
fn blank(out: &mut [char], start: usize, end: usize) {
    let stop = end.min(out.len());
    for slot in out.iter_mut().take(stop).skip(start) {
        if *slot != '\n' {
            *slot = ' ';
        }
    }
}

/// The index of the first `target` in `chars` at or after `from`, else `chars.len()`.
fn find_char_or_end(chars: &[char], target: char, from: usize) -> usize {
    match chars
        .iter()
        .skip(from)
        .position(|candidate| *candidate == target)
    {
        Some(offset) => from.saturating_add(offset),
        None => chars.len(),
    }
}

/// The text with every comment and literal blanked, offsets preserved.
///
/// A `use` statement or a bare `name::` path can only be code, so the scan has to
/// tell code from a string or comment that happens to spell one. This is a lexer
/// for exactly the Rust forms that can hide code-like text: line and nested
/// block comments, string and byte-string literals with escapes, raw strings
/// with any number of `#`, and character literals, which are told apart from
/// lifetimes by their closing quote. Every blanked character becomes a space and
/// every newline is kept, so an offset and a line number mean the same thing in
/// the result as in the file.
pub(crate) fn code_only(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    let length = chars.len();
    let mut index = 0_usize;
    while index < length {
        let ch = chars[index];
        let after = chars.get(index.saturating_add(1)).copied();
        if ch == '/' && after == Some('/') {
            let end = find_char_or_end(&chars, '\n', index);
            blank(&mut out, index, end);
            index = end;
        } else if ch == '/' && after == Some('*') {
            let mut depth = 1_usize;
            let mut cursor = index.saturating_add(2);
            while cursor < length && depth > 0 {
                let pair = (chars.get(cursor), chars.get(cursor.saturating_add(1)));
                if pair == (Some(&'/'), Some(&'*')) {
                    depth = depth.saturating_add(1);
                    cursor = cursor.saturating_add(2);
                } else if pair == (Some(&'*'), Some(&'/')) {
                    depth = depth.saturating_sub(1);
                    cursor = cursor.saturating_add(2);
                } else {
                    cursor = cursor.saturating_add(1);
                }
            }
            blank(&mut out, index, cursor);
            index = cursor;
        } else if ch == 'r'
            && (index == 0
                || chars
                    .get(index.saturating_sub(1))
                    .is_some_and(|before| !is_word(*before) || *before == 'b'))
        {
            let mut hashes = 0_usize;
            while chars.get(index.saturating_add(1).saturating_add(hashes)) == Some(&'#') {
                hashes = hashes.saturating_add(1);
            }
            let quote_at = index.saturating_add(1).saturating_add(hashes);
            if chars.get(quote_at) == Some(&'"') {
                let mut closing = vec!['"'];
                closing.extend(std::iter::repeat_n('#', hashes));
                let from = index.saturating_add(2).saturating_add(hashes);
                let end = match find_sequence(&chars, &closing, from) {
                    Some(found) => found.saturating_add(closing.len()),
                    None => length,
                };
                blank(&mut out, index, end);
                index = end;
            } else {
                index = index.saturating_add(1);
            }
        } else if ch == '"' {
            let mut cursor = index.saturating_add(1);
            while cursor < length && chars[cursor] != '"' {
                cursor = cursor.saturating_add(if chars[cursor] == '\\' { 2 } else { 1 });
            }
            blank(&mut out, index, cursor.saturating_add(1));
            index = cursor.saturating_add(1);
        } else if ch == '\'' {
            if after == Some('\\') {
                let from = if chars.get(index.saturating_add(2)) == Some(&'\'') {
                    index.saturating_add(3)
                } else {
                    index.saturating_add(2)
                };
                let end = match chars
                    .iter()
                    .skip(from)
                    .position(|candidate| *candidate == '\'')
                {
                    Some(offset) => from.saturating_add(offset).saturating_add(1),
                    None => length,
                };
                blank(&mut out, index, end);
                index = end;
            } else if index.saturating_add(2) < length
                && chars[index.saturating_add(2)] == '\''
                && after != Some('\n')
            {
                blank(&mut out, index, index.saturating_add(3));
                index = index.saturating_add(3);
            } else {
                index = index.saturating_add(1);
            }
        } else {
            index = index.saturating_add(1);
        }
    }
    out.into_iter().collect()
}

/// Whether an attribute's text gates its item on `test`: `cfg(test)`, or a
/// `cfg(all(..))` with `test` among its top-level arguments. `cfg(not(test))`
/// and `cfg(any(test, ..))` compile in a shipped build, so they gate nothing.
pub(crate) fn gates_on_test(body: &str) -> bool {
    let compact: String = body.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact == "cfg(test)" {
        return true;
    }
    let Some(inner) = compact
        .strip_prefix("cfg(all(")
        .and_then(|rest| rest.strip_suffix("))"))
    else {
        return false;
    };
    let mut depth = 0_usize;
    let mut argument = String::new();
    for ch in inner.chars().chain(std::iter::once(',')) {
        match ch {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if ch == ',' && depth == 0 {
            if argument == "test" {
                return true;
            }
            argument.clear();
        } else {
            argument.push(ch);
        }
    }
    false
}

/// One `#[..]` or `#![..]` attribute of a blanked source.
pub(crate) struct Attribute {
    /// Whether it is an inner attribute, `#![..]`.
    pub(crate) inner: bool,
    /// The index of its `#`.
    pub(crate) start: usize,
    /// The text between the brackets.
    pub(crate) body: String,
    /// The index just past its closing `]`.
    pub(crate) end: usize,
}

/// Every attribute of `chars`, which must already be `code_only` text.
pub(crate) fn attributes(chars: &[char]) -> Vec<Attribute> {
    let mut found = Vec::new();
    for (start, ch) in chars.iter().enumerate() {
        if *ch != '#' {
            continue;
        }
        let inner = chars.get(start.saturating_add(1)) == Some(&'!');
        let open = start.saturating_add(if inner { 2 } else { 1 });
        if chars.get(open) != Some(&'[') {
            continue;
        }
        let mut depth = 0_usize;
        let mut close = None;
        for (index, bracket) in chars.iter().enumerate().skip(open) {
            match *bracket {
                '[' => depth = depth.saturating_add(1),
                ']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            break;
        };
        let body = chars
            .iter()
            .take(close)
            .skip(open.saturating_add(1))
            .collect();
        found.push(Attribute {
            inner,
            start,
            body,
            end: close.saturating_add(1),
        });
    }
    found
}

/// The index just past the item that starts at `from`: its braces, or the `;`
/// outside any parentheses and brackets (`fn f(a: [u8; 4]);` ends at the last one).
pub(crate) fn item_end(chars: &[char], from: usize) -> usize {
    let mut nesting = 0_usize;
    let mut braces = 0_usize;
    for (index, ch) in chars.iter().enumerate().skip(from) {
        match *ch {
            '(' | '[' => nesting = nesting.saturating_add(1),
            ')' | ']' => nesting = nesting.saturating_sub(1),
            ';' if nesting == 0 && braces == 0 => return index.saturating_add(1),
            '{' if nesting == 0 => braces = braces.saturating_add(1),
            '}' if nesting == 0 => {
                braces = braces.saturating_sub(1);
                if braces == 0 {
                    return index.saturating_add(1);
                }
            }
            _ => {}
        }
    }
    chars.len()
}
