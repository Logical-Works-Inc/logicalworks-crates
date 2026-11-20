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

/// The advisory's title when it has no `title` key: the first line after the
/// frontmatter, when it is an `# ` heading. advisory-db writes the title there
/// and no key (format v3), so a reader of the key alone titles every live
/// advisory `None` (#955).
fn heading_title(text: &str, frontmatter: &str) -> Option<String> {
    let start = text.find(frontmatter)?.saturating_add(frontmatter.len());
    let mut body = text.get(start..)?.lines().map(str::trim);
    let mut line = body.next()?;
    if matches!(line, "```" | "---") {
        line = body.next()?;
    }
    while line.is_empty() {
        line = body.next()?;
    }
    let title = line.strip_prefix("# ")?.trim();
    (!title.is_empty()).then(|| title.to_owned())
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

/// An array value whose closing bracket has not been read yet: the key it
/// belongs to and the items collected so far.
type PendingArray = (&'static str, Vec<String>);

/// Every value the frontmatter wrote, before any of it is judged.
#[derive(Default)]
struct Scanned {
    /// `[advisory] id`.
    id: Option<String>,
    /// `[advisory] package`.
    package: Option<String>,
    /// `[advisory] title`.
    title: Option<String>,
    /// `[advisory] date`.
    date: Option<String>,
    /// `[advisory] informational`.
    informational: Option<String>,
    /// `[advisory] withdrawn`.
    withdrawn: Option<String>,
    /// `[versions] patched`.
    patched: Vec<String>,
    /// `[versions] unaffected`.
    unaffected: Vec<String>,
    /// `[affected]` os and arch.
    affected: Affected,
}

impl Scanned {
    /// Reads every line of the frontmatter.
    fn read(frontmatter: &str) -> Self {
        let mut scanned = Self::default();
        let mut section = FrontmatterSection::None;
        let mut in_array: Option<PendingArray> = None;
        for raw_line in frontmatter.lines() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(pending) = in_array.take() {
                in_array = scanned.continue_array(pending, line);
            } else if let Some(header) = section_header(line) {
                section = header;
            } else if let Some((key_raw, val_raw)) = line.split_once('=') {
                in_array = scanned.assign(section, &unquote(key_raw), val_raw.trim());
            }
        }
        scanned
    }

    /// Reads one line of a multi-line array; the array stays pending until a
    /// line carries its closing bracket.
    fn continue_array(&mut self, pending: PendingArray, line: &str) -> Option<PendingArray> {
        let (target, mut items) = pending;
        let Some((inside, _)) = line.split_once(']') else {
            parse_items(line, &mut items);
            return Some((target, items));
        };
        parse_items(inside, &mut items);
        match target {
            "patched" => self.patched.extend(items),
            "unaffected" => self.unaffected.extend(items),
            "os" => self.affected.os.extend(items),
            "arch" => self.affected.arch.extend(items),
            _ => {}
        }
        None
    }

    /// Reads one `key = value` line of `section`; an array that opens on this
    /// line and closes on a later one comes back pending.
    fn assign(
        &mut self,
        section: FrontmatterSection,
        key: &str,
        val: &str,
    ) -> Option<PendingArray> {
        match (section, key) {
            (FrontmatterSection::Advisory, "id") => self.id = Some(unquote(val)),
            (FrontmatterSection::Advisory, "package") => self.package = Some(unquote(val)),
            (FrontmatterSection::Advisory, "title") => self.title = Some(unquote(val)),
            (FrontmatterSection::Advisory, "date") => self.date = Some(unquote(val)),
            (FrontmatterSection::Advisory, "informational") => {
                self.informational = Some(unquote(val));
            }
            (FrontmatterSection::Advisory, "withdrawn") => self.withdrawn = Some(unquote(val)),
            (FrontmatterSection::Versions, "patched") => {
                return process_array_syntax(val, "patched", &mut self.patched);
            }
            (FrontmatterSection::Versions, "unaffected") => {
                return process_array_syntax(val, "unaffected", &mut self.unaffected);
            }
            (FrontmatterSection::Affected, "os") => {
                return process_array_syntax(val, "os", &mut self.affected.os);
            }
            (FrontmatterSection::Affected, "arch") => {
                return process_array_syntax(val, "arch", &mut self.affected.arch);
            }
            _ => {}
        }
        None
    }
}

/// The section a `[header]` line opens, or `None` when the line is not a header.
fn section_header(line: &str) -> Option<FrontmatterSection> {
    let inside = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(match inside.trim() {
        "advisory" => FrontmatterSection::Advisory,
        "versions" => FrontmatterSection::Versions,
        "affected" => FrontmatterSection::Affected,
        _ => FrontmatterSection::Other,
    })
}

/// Parses the frontmatter text of an advisory Markdown file into an [`Advisory`].
pub(crate) fn parse_advisory(path: &Path, text: &str) -> Result<Advisory, AdvisoryError> {
    let frontmatter = match extract_frontmatter(text) {
        Some(extracted_fm) => extracted_fm,
        None => return err_frontmatter(path, "no frontmatter block found"),
    };

    let Scanned {
        id: id_opt,
        package: package_opt,
        title: title_opt,
        date: date_opt,
        informational: informational_opt,
        withdrawn: withdrawn_opt,
        patched,
        unaffected,
        affected,
    } = Scanned::read(frontmatter);

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
    let title_str = title_opt
        .filter(|t_val| !t_val.trim().is_empty())
        .or_else(|| heading_title(text, frontmatter));

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
