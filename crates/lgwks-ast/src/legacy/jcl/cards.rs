//! The JCL cards: comments dropped, continuations joined, instream skipped.
//!
//! A card is one `//name OPERATION parameters` statement with its span.
//! Continuations join at a trailing comma onto the next `//` line; `//*`
//! comments never break a continuation. After a `DD *` or `DD DATA` card the
//! instream data skips through to its `/*` end: instream bytes are input,
//! not structure.

use super::super::error::LegacyError;
use super::super::{LegacyLimits, uppercased};

/// One JCL card: its operation words and its span.
pub(crate) struct Card {
    /// The card name, or `None` for a nameless card.
    pub(crate) name: Option<String>,
    /// The operation word, folded to uppercase.
    pub(crate) operation: String,
    /// The parameter words after the operation, joined with one blank.
    pub(crate) params: String,
    /// The first physical line.
    pub(crate) start: usize,
    /// The last physical line.
    pub(crate) end: usize,
}

/// Reads `source` into cards: comments dropped, continuations joined,
/// instream data skipped through to its end.
pub(crate) fn read_cards(source: &str, limits: &LegacyLimits) -> Result<Vec<Card>, LegacyError> {
    let mut cards: Vec<Card> = Vec::new();
    let mut pending: Option<Card> = None;
    let mut continued = false;
    let mut instream = false;
    let mut last_line = 0_usize;
    for (offset, raw) in source.lines().enumerate() {
        let line_no = offset.saturating_add(1);
        last_line = line_no;
        limits.check_lines(line_no)?;
        limits.check_nodes(cards.len().saturating_add(1))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if instream {
            if trimmed == "/*" || trimmed.starts_with("//") {
                instream = false;
            } else {
                continue;
            }
        }
        if !trimmed.starts_with("//") {
            let refusal = Err(LegacyError::UnknownStatement {
                line: line_no,
                text: String::from(trimmed),
            });
            tracing::debug!(line_no, "jcl: refusing a line outside a card");
            return refusal;
        }
        if trimmed.starts_with("//*") {
            continue;
        }
        let body = trimmed.trim_start_matches('/').trim_start();
        if body.is_empty() {
            if continued {
                let refusal = Err(LegacyError::UnknownStatement {
                    line: line_no,
                    text: String::from("a null line cannot continue a card"),
                });
                tracing::debug!(line_no, "jcl: refusing a null continuation line");
                return refusal;
            }
            continued = false;
            pending = None;
            continue;
        }
        if continued {
            match pending.as_mut() {
                Some(card) => {
                    card.params.push(' ');
                    card.params.push_str(body);
                    card.end = line_no;
                }
                None => {
                    let refusal = Err(LegacyError::UnknownStatement {
                        line: line_no,
                        text: String::from(trimmed),
                    });
                    tracing::debug!(line_no, "jcl: refusing a continuation with no card");
                    return refusal;
                }
            }
        } else {
            let (name, operation, params) = split_card(body);
            if let Some(card) = pending.take() {
                cards.push(card);
            }
            pending = Some(Card {
                name,
                operation,
                params,
                start: line_no,
                end: line_no,
            });
        }
        continued = trimmed.ends_with(',');
        if !continued && let Some(card) = pending.take() {
            if is_instream_dd(&card) {
                instream = true;
            }
            cards.push(card);
        }
    }
    if continued {
        let refusal = Err(LegacyError::UnknownStatement {
            line: last_line,
            text: String::from("the job ends on a continuation line"),
        });
        tracing::debug!("jcl: refusing a job that ends on a continuation");
        return refusal;
    }
    if let Some(card) = pending.take() {
        cards.push(card);
    }
    Ok(cards)
}

/// Splits a card body into its name, its operation, and its parameter text.
///
/// A body of one word is a nameless card running that operation; a body whose
/// second word carries `=` is a nameless card too, its parameters starting
/// there; otherwise the first word names the card.
fn split_card(body: &str) -> (Option<String>, String, String) {
    let mut words = body.split_whitespace();
    let first = match words.next() {
        Some(word) => word,
        None => return (None, String::new(), String::new()),
    };
    let second = match words.next() {
        Some(word) => word,
        None => return (None, uppercased(first), String::new()),
    };
    if second.contains('=') {
        let mut params = String::from(second);
        for word in words {
            params.push(' ');
            params.push_str(word);
        }
        return (None, uppercased(first), params);
    }
    let rest: Vec<&str> = words.collect();
    let mut params = String::new();
    for word in rest.iter() {
        if !params.is_empty() {
            params.push(' ');
        }
        params.push_str(word);
    }
    (Some(uppercased(first)), uppercased(second), params)
}

/// Whether the DD card starts instream data: `*` or DATA past the operation.
fn is_instream_dd(card: &Card) -> bool {
    if card.operation != "DD" {
        return false;
    }
    let upper = uppercased(card.params.as_str());
    upper == "*" || upper == "DATA" || upper.starts_with("*,") || upper.starts_with("DATA,")
}
