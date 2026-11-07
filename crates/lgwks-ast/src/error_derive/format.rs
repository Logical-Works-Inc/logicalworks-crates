//! String interpolation and format attribute handling for `#[derive(Error)]` (#842).

use proc_macro2::{Literal, Span, TokenStream};

/// Appends a field placeholder to `out`.
fn push_field_placeholder(out: &mut String, idx: usize, spec_part: &str) {
    out.push_str("{__field_");
    out.push_str(&idx.to_string());
    out.push_str(spec_part);
    out.push('}');
}

/// Appends character twice to out.
fn push_pair(out: &mut String, ch: char) {
    out.push(ch);
    out.push(ch);
}

fn validate_and_push_index(
    out: &mut String,
    idx: usize,
    field_count: usize,
    spec_part: &str,
    is_auto: bool,
) -> Result<(), String> {
    if idx >= field_count {
        let label = if is_auto { "positional index" } else { "index" };
        tracing::debug!(idx, field_count, "out of bounds format index");
        return Err(format!("{label} {idx} >= {field_count}"));
    }
    push_field_placeholder(out, idx, spec_part);
    Ok(())
}

/// Format attribute details.
#[derive(Clone)]
pub(super) struct FormatAttr {
    /// Unescaped format string text.
    pub(super) format: String,
    /// Original literal token.
    pub(super) literal: Literal,
    /// Extra format arguments, if any.
    pub(super) extra_args: Option<TokenStream>,
    /// Span of the attribute.
    pub(super) span: Span,
}

/// Unescape a string literal without external dependencies.
pub(super) fn unescape_string_literal(lit_str: &str) -> Option<String> {
    if lit_str.starts_with('r') {
        let first_quote = lit_str.find('"')?;
        let last_quote = lit_str.rfind('"')?;
        if first_quote >= last_quote {
            return None;
        }
        return lit_str
            .get(first_quote.saturating_add(1)..last_quote)
            .map(|sub| sub.to_owned());
    }

    if lit_str.starts_with('"') && lit_str.ends_with('"') && lit_str.len() >= 2 {
        let content = lit_str.get(1..lit_str.len().saturating_sub(1))?;
        let mut out = String::with_capacity(content.len());
        let mut chars = content.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let esc_ch = chars.next()?;
                match esc_ch {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '0' => out.push('\0'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    '\n' => {
                        while let Some(next_ref) = chars.peek() {
                            let next_ch = *next_ref;
                            if next_ch.is_whitespace() {
                                let _ = chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push(ch);
            }
        }
        return Some(out);
    }

    None
}

/// Rewrite `{0}`, `{1}`, `{}` placeholders in `fmt` to `{__field_0}`, `{__field_1}`, etc.
pub(super) fn rewrite_format_string(
    fmt: &str,
    is_tuple: bool,
    field_names: &[String],
    field_count: usize,
) -> Result<String, String> {
    let mut out = String::with_capacity(fmt.len().saturating_add(16));
    let mut chars = fmt.chars().peekable();
    let mut auto_idx: usize = 0;

    while let Some(ch) = chars.next() {
        if ch == '{' {
            if chars.peek() == Some(&'{') {
                let _ = chars.next();
                push_pair(&mut out, '{');
                continue;
            }
            let mut inside = String::new();
            let mut closed = false;
            for inner_ch in chars.by_ref() {
                if inner_ch == '}' {
                    closed = true;
                    break;
                }
                inside.push(inner_ch);
            }
            if !closed {
                tracing::debug!("unclosed '{{' in format string");
                return Err("unclosed '{' in format string".to_owned());
            }

            let (arg_part, spec_part) = match inside.find(':') {
                Some(idx) => {
                    let mut arg_slice: &str = "";
                    if let Some(slice) = inside.get(..idx) {
                        arg_slice = slice;
                    }
                    let mut spec_slice: &str = "";
                    if let Some(slice) = inside.get(idx..) {
                        spec_slice = slice;
                    }
                    (arg_slice.trim(), spec_slice)
                }
                None => (inside.trim(), ""),
            };

            if arg_part.is_empty() {
                validate_and_push_index(&mut out, auto_idx, field_count, spec_part, true)?;
                auto_idx = auto_idx.saturating_add(1);
            } else if let Ok(num) = arg_part.parse::<usize>() {
                validate_and_push_index(&mut out, num, field_count, spec_part, false)?;
            } else {
                if is_tuple {
                    tracing::debug!(
                        arg_part,
                        "named field used in format string for tuple variant"
                    );
                    return Err(format!(
                        "named field '{arg_part}' used in format string for tuple variant"
                    ));
                }
                if !field_names.iter().any(|name_item| name_item == arg_part) {
                    tracing::debug!(arg_part, "unknown field referenced in format string");
                    return Err(format!(
                        "unknown field '{arg_part}' referenced in format string"
                    ));
                }
                out.push('{');
                out.push_str(arg_part);
                out.push_str(spec_part);
                out.push('}');
            }
        } else if ch == '}' {
            if chars.peek() == Some(&'}') {
                let _ = chars.next();
                push_pair(&mut out, '}');
                continue;
            }
            tracing::debug!("unmatched '}}' in format string");
            return Err("unmatched '}' in format string".to_owned());
        } else {
            out.push(ch);
        }
    }

    Ok(out)
}
