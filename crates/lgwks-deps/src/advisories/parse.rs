//! Advisory Markdown & Frontmatter Parsing for RustSec (#799).

use std::path::Path;

use super::types::{Advisory, AdvisoryError, AdvisoryKind, Affected, err_frontmatter};

#[cfg(test)]
mod sim_parse;

/// Unquotes a TOML string literal and trims surrounding whitespace.
pub(super) fn unquote(input_str: &str) -> String {
    let trimmed = input_str.trim();
    if ((trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
        && trimmed.len() >= 2
    {
        let mut inner = "";
        if let Some(inner_str) = trimmed.get(1..trimmed.len().saturating_sub(1)) {
            inner = inner_str;
        }
        return inner.to_owned();
    }
    trimmed.to_owned()
}

/// Strips comments (`#`) from a TOML line outside quoted strings.
pub(super) fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    let mut quote_char = ' ';
    for (idx, ch) in line.char_indices() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch == '#' {
            let mut prefix = "";
            if let Some(sub_str) = line.get(..idx) {
                prefix = sub_str;
            }
            return prefix;
        }
    }
    line
}

/// Finds the byte offset of the closing fence delimiter line.
fn find_fence_end(text: &str, delimiter: &str) -> Option<usize> {
    let mut current_offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed_line = line.trim_end_matches(['\r', '\n']).trim();
        if trimmed_line == delimiter {
            return Some(current_offset);
        }
        current_offset = current_offset.saturating_add(line.len());
    }
    None
}

/// Strips newline prefixes and returns fence content up to `closer`.
fn strip_fence_content<'a>(rest: &'a str, closer: &str) -> Option<&'a str> {
    let mut after_opener = rest;
    if let Some(stripped) = rest.strip_prefix("\r\n") {
        after_opener = stripped;
    } else if let Some(stripped) = rest.strip_prefix('\n') {
        after_opener = stripped;
    }
    let end_idx = find_fence_end(after_opener, closer)?;
    after_opener.get(..end_idx)
}

/// Extracts the frontmatter block from an advisory markdown string.
fn extract_frontmatter(text: &str) -> Option<&str> {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("```toml") {
        strip_fence_content(rest, "```")
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        strip_fence_content(rest, "```")
    } else if let Some(rest) = trimmed.strip_prefix("---") {
        strip_fence_content(rest, "---")
    } else if trimmed.starts_with("[advisory]") {
        let mut byte_len: usize = 0;
        for line in trimmed.split_inclusive('\n') {
            let line_trimmed = line.trim();
            if line_trimmed.starts_with('#')
                || line_trimmed.starts_with("```")
                || line_trimmed.starts_with("---")
            {
                break;
            }
            byte_len = byte_len.saturating_add(line.len());
        }
        trimmed.get(..byte_len).or(Some(trimmed))
    } else {
        None
    }
}

/// Current section within frontmatter TOML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrontmatterSection {
    /// Before any section header.
    None,
    /// `[advisory]` table.
    Advisory,
    /// `[versions]` table.
    Versions,
    /// `[affected]` table; `[affected.functions]` is [`Self::Other`].
    Affected,
    /// Another uninspected table.
    Other,
}

/// Splits a TOML array body on its top-level commas and appends each item.
///
/// A comma inside a quoted string belongs to that string: RustSec writes
/// `patched = [">= 1.0.1, < 1.1.0", ">= 1.1.3"]`, where the first item is one
/// conjunction. Splitting it would make each half its own alternative, and
/// `< 1.1.0` or `>= 1.0.1` between them matches every version, so a vulnerable
/// lock would read as patched.
fn parse_items(source: &str, out: &mut Vec<String>) {
    let mut push_item = |raw: &str| {
        let cleaned = unquote(raw);
        if !cleaned.is_empty() {
            out.push(cleaned);
        }
    };
    let mut open_quote: Option<char> = None;
    let mut start = 0_usize;
    for (idx, ch) in source.char_indices() {
        match open_quote {
            Some(quote) => {
                if ch == quote {
                    open_quote = None;
                }
            }
            None if ch == '"' || ch == '\'' => open_quote = Some(ch),
            None if ch == ',' => {
                if let Some(item) = source.get(start..idx) {
                    push_item(item);
                }
                start = idx.saturating_add(1);
            }
            None => {}
        }
    }
    if let Some(item) = source.get(start..) {
        push_item(item);
    }
}

/// Parses an inline or multi-line TOML array syntax value.
fn process_array_syntax(
    val: &str,
    target_name: &'static str,
    target: &mut Vec<String>,
) -> Option<(&'static str, Vec<String>)> {
    if let Some(stripped) = val.strip_prefix('[') {
        if let Some(pos) = stripped.find(']') {
            let mut inside = "";
            if let Some(sub_str) = stripped.get(..pos) {
                inside = sub_str;
            }
            parse_items(inside, target);
            None
        } else {
            let mut items = Vec::new();
            parse_items(stripped, &mut items);
            Some((target_name, items))
        }
    } else {
        let cleaned = unquote(val);
        if !cleaned.is_empty() {
            target.push(cleaned);
        }
        None
    }
}

/// Parses the frontmatter text of an advisory Markdown file into an [`Advisory`].
pub(crate) fn parse_advisory(path: &Path, text: &str) -> Result<Advisory, AdvisoryError> {
    let frontmatter = match extract_frontmatter(text) {
        Some(extracted_fm) => extracted_fm,
        None => return err_frontmatter(path, "no frontmatter block found"),
    };

    let mut section = FrontmatterSection::None;
    let mut in_array: Option<(&'static str, Vec<String>)> = None;

    let mut id_opt: Option<String> = None;
    let mut package_opt: Option<String> = None;
    let mut title_opt: Option<String> = None;
    let mut date_opt: Option<String> = None;
    let mut informational_opt: Option<String> = None;
    let mut withdrawn_opt: Option<String> = None;
    let mut patched: Vec<String> = Vec::new();
    let mut unaffected: Vec<String> = Vec::new();
    let mut affected = Affected::default();

    for raw_line in frontmatter.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some((target, mut items)) = in_array.take() {
            if let Some(pos) = line.find(']') {
                let mut inside = "";
                if let Some(sub_str) = line.get(..pos) {
                    inside = sub_str;
                }
                parse_items(inside, &mut items);
                match target {
                    "patched" => patched.extend(items),
                    "unaffected" => unaffected.extend(items),
                    "os" => affected.os.extend(items),
                    "arch" => affected.arch.extend(items),
                    _ => {}
                }
            } else {
                parse_items(line, &mut items);
                in_array = Some((target, items));
            }
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let mut inside = "";
            if let Some(sub_str) = line.get(1..line.len().saturating_sub(1)) {
                inside = sub_str;
            }
            let header = inside.trim();
            match header {
                "advisory" => section = FrontmatterSection::Advisory,
                "versions" => section = FrontmatterSection::Versions,
                "affected" => section = FrontmatterSection::Affected,
                _ => section = FrontmatterSection::Other,
            }
            continue;
        }

        if let Some((key_raw, val_raw)) = line.split_once('=') {
            let key = unquote(key_raw);
            let val = val_raw.trim();

            match section {
                FrontmatterSection::Advisory => match key.as_str() {
                    "id" => id_opt = Some(unquote(val)),
                    "package" => package_opt = Some(unquote(val)),
                    "title" => title_opt = Some(unquote(val)),
                    "date" => date_opt = Some(unquote(val)),
                    "informational" => informational_opt = Some(unquote(val)),
                    "withdrawn" => withdrawn_opt = Some(unquote(val)),
                    _ => {}
                },
                FrontmatterSection::Versions => match key.as_str() {
                    "patched" => {
                        if let Some(pending) = process_array_syntax(val, "patched", &mut patched) {
                            in_array = Some(pending);
                        }
                    }
                    "unaffected" => {
                        if let Some(pending) =
                            process_array_syntax(val, "unaffected", &mut unaffected)
                        {
                            in_array = Some(pending);
                        }
                    }
                    _ => {}
                },
                FrontmatterSection::Affected => match key.as_str() {
                    "os" => {
                        if let Some(pending) = process_array_syntax(val, "os", &mut affected.os) {
                            in_array = Some(pending);
                        }
                    }
                    "arch" => {
                        if let Some(pending) = process_array_syntax(val, "arch", &mut affected.arch)
                        {
                            in_array = Some(pending);
                        }
                    }
                    _ => {}
                },
                FrontmatterSection::None | FrontmatterSection::Other => {}
            }
        }
    }

    let advisory_id = match id_opt {
        Some(val) if !val.is_empty() => val,
        _ => return err_frontmatter(path, "missing required field 'id' in [advisory]"),
    };

    let package_name = match package_opt {
        Some(val) if !val.is_empty() => val,
        _ => return err_frontmatter(path, "missing required field 'package' in [advisory]"),
    };

    let kind = match informational_opt {
        Some(ref info) if !info.trim().is_empty() => {
            AdvisoryKind::Informational(info.trim().to_owned())
        }
        _ => AdvisoryKind::Vulnerability,
    };

    let withdrawn_date = withdrawn_opt.filter(|w_val| !w_val.trim().is_empty());
    let pub_date = date_opt.filter(|d_val| !d_val.trim().is_empty());
    let title_str = title_opt.filter(|t_val| !t_val.trim().is_empty());

    let mut advisory = Advisory::new(
        advisory_id,
        package_name,
        pub_date,
        withdrawn_date,
        kind,
        patched,
        unaffected,
    )
    .with_affected(affected);
    if let Some(title_val) = title_str {
        advisory = advisory.with_title(title_val);
    }

    Ok(advisory)
}
