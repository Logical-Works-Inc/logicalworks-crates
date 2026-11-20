//! The copybook clause grammar: items, `OCCURS` and `PIC`.
//!
//! [`parse_item`] turns the tokens after `LEVEL NAME` into one [`Item`];
//! only the documented clauses are admitted, and anything else is a located
//! diagnostic.
//!
//! [`Item`]: super::Item

use super::model::CopybookError;
use super::{Item, Occurs, Picture, Usage};

/// Parses the clause tokens after `LEVEL NAME` into one item.
pub(crate) fn parse_item(
    line_no: usize,
    level: u8,
    name: String,
    tokens: &[&str],
) -> Result<Item, CopybookError> {
    let mut picture: Option<Picture> = None;
    let mut occurs: Option<Occurs> = None;
    let mut redefines: Option<String> = None;
    let mut index: usize = 0;
    while index < tokens.len() {
        let Some(word) = tokens.get(index) else {
            break;
        };
        if *word == "PIC" || *word == "PICTURE" {
            let Some(pattern) = tokens.get(index.saturating_add(1)) else {
                let refusal = Err(CopybookError::BadPic {
                    line: line_no,
                    field: name,
                    text: String::from("PIC with no picture"),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a bare PIC");
                return refusal;
            };
            let mut after = index.saturating_add(2);
            let mut usage = Usage::Display;
            if let Some(next) = tokens.get(after) {
                if *next == "COMP-3" {
                    usage = Usage::Packed;
                    after = after.saturating_add(1);
                } else if *next == "COMP" || *next == "COMP-4" || *next == "BINARY" {
                    usage = Usage::Binary;
                    after = after.saturating_add(1);
                }
            }
            picture = Some(parse_picture(line_no, &name, pattern, usage)?);
            index = after;
        } else if *word == "OCCURS" {
            let (parsed, after) = parse_occurs(line_no, &name, tokens, index.saturating_add(1))?;
            occurs = Some(parsed);
            index = after;
        } else if *word == "REDEFINES" {
            let Some(target) = tokens.get(index.saturating_add(1)) else {
                let refusal = Err(CopybookError::UnknownRedefines {
                    line: line_no,
                    field: name,
                    name: String::from("<missing>"),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a bare REDEFINES");
                return refusal;
            };
            redefines = Some(String::from(*target));
            index = index.saturating_add(2);
        } else {
            let refusal = Err(CopybookError::UnknownClause {
                line: line_no,
                text: String::from(*word),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(line_no, "copybook: refusing an unknown clause");
            return refusal;
        }
    }
    Ok(Item {
        line_no,
        level,
        name,
        picture,
        occurs,
        redefines,
    })
}

/// Parses an `OCCURS` tail starting after the keyword.
fn parse_occurs(
    line_no: usize,
    field: &str,
    tokens: &[&str],
    at: usize,
) -> Result<(Occurs, usize), CopybookError> {
    let refused = |text: String| CopybookError::BadOccurs {
        line: line_no,
        field: String::from(field),
        text,
    };
    let Some(first) = tokens.get(at) else {
        let refusal = Err(refused(String::from("OCCURS with no count")));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing a bare OCCURS");
        return refusal;
    };
    let Ok(min) = first.parse::<usize>() else {
        let refusal = Err(refused(String::from(*first)));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing a non-numeric OCCURS count");
        return refusal;
    };
    // A fixed table needs one occurrence; `0 TO n DEPENDING ON` may hold none
    // (Enterprise COBOL Language Reference, OCCURS DEPENDING ON: integer-1
    // may be zero).
    if min == 0 && tokens.get(at.saturating_add(1)) != Some(&"TO") {
        let refusal = Err(refused(String::from(*first)));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing a zero fixed OCCURS count");
        return refusal;
    }
    let after = at.saturating_add(1);
    let Some(second) = tokens.get(after) else {
        return Ok((
            Occurs {
                min,
                max: min,
                depending_on: None,
            },
            after,
        ));
    };
    if *second == "TIMES" {
        return Ok((
            Occurs {
                min,
                max: min,
                depending_on: None,
            },
            after.saturating_add(1),
        ));
    }
    if *second != "TO" {
        return Ok((
            Occurs {
                min,
                max: min,
                depending_on: None,
            },
            after,
        ));
    }
    parse_occurs_range(line_no, field, tokens, after.saturating_add(1), min)
}

/// Parses the `m TO n [TIMES] DEPENDING ON field` tail of an `OCCURS`, from
/// the token after `TO`, returning the clause and the next unread token.
fn parse_occurs_range(
    line_no: usize,
    field: &str,
    tokens: &[&str],
    start: usize,
    min: usize,
) -> Result<(Occurs, usize), CopybookError> {
    let refused = |text: String| CopybookError::BadOccurs {
        line: line_no,
        field: String::from(field),
        text,
    };
    let mut after = start;
    let Some(third) = tokens.get(after) else {
        let refusal = Err(refused(String::from("OCCURS m TO with no ceiling")));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing an OCCURS TO with no ceiling");
        return refusal;
    };
    let Ok(max) = third.parse::<usize>() else {
        let refusal = Err(refused(String::from(*third)));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing a non-numeric OCCURS ceiling");
        return refusal;
    };
    if max < min {
        let refusal = Err(refused(String::from(*third)));
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            line_no,
            "copybook: refusing an OCCURS ceiling below its floor"
        );
        return refusal;
    }
    after = after.saturating_add(1);
    if tokens.get(after) == Some(&"TIMES") {
        after = after.saturating_add(1);
    }
    let Some(depending) = tokens.get(after) else {
        let refusal = Err(refused(String::from("OCCURS m TO n with no DEPENDING ON")));
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            line_no,
            "copybook: refusing an OCCURS range with no DEPENDING ON"
        );
        return refusal;
    };
    if *depending != "DEPENDING" {
        let refusal = Err(refused(String::from(*depending)));
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            line_no,
            "copybook: refusing an OCCURS range without DEPENDING"
        );
        return refusal;
    }
    after = after.saturating_add(1);
    if tokens.get(after) != Some(&"ON") {
        let refusal = Err(refused(String::from("DEPENDING with no ON")));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing DEPENDING with no ON");
        return refusal;
    }
    after = after.saturating_add(1);
    let Some(holder) = tokens.get(after) else {
        let refusal = Err(refused(String::from("DEPENDING ON with no field")));
        #[cfg(feature = "trace")]
        crate::trace::debug!(line_no, "copybook: refusing DEPENDING ON with no field");
        return refusal;
    };
    after = after.saturating_add(1);
    Ok((
        Occurs {
            min,
            max,
            depending_on: Some(String::from(*holder)),
        },
        after,
    ))
}

/// Reads the digits of a `(n)` repeat count whose `(` is already consumed,
/// leaving `index` past the `)`. `None` for an unterminated, non-numeric or
/// zero count.
fn parenthesised_count(chars: &[char], index: &mut usize) -> Option<usize> {
    let mut number = String::new();
    while let Some(&drawn) = chars.get(*index) {
        if drawn == ')' {
            break;
        }
        number.push(drawn);
        *index = index.saturating_add(1);
    }
    if chars.get(*index) != Some(&')') {
        return None;
    }
    *index = index.saturating_add(1);
    match number.parse::<usize>() {
        Ok(parsed) if parsed > 0 => Some(parsed),
        _ => None,
    }
}

/// Parses one picture string into a [`Picture`].
///
/// [`Picture`]: super::Picture
fn parse_picture(
    line_no: usize,
    field: &str,
    pattern: &str,
    usage: Usage,
) -> Result<Picture, CopybookError> {
    let refused = |text: &str| CopybookError::BadPic {
        line: line_no,
        field: String::from(field),
        text: String::from(text),
    };
    let chars: Vec<char> = pattern.chars().collect();
    let mut index: usize = 0;
    let mut signed = false;
    if chars.first() == Some(&'S') {
        signed = true;
        index = index.saturating_add(1);
    }
    let mut text_len: Option<usize> = None;
    let mut digits: usize = 0;
    let mut scale: u32 = 0;
    let mut seen_scale = false;
    let mut seen_kind: Option<char> = None;
    while index < chars.len() {
        let Some(kind) = chars.get(index) else {
            break;
        };
        let known = matches!(*kind, 'X' | '9' | 'V');
        let second_v = *kind == 'V' && seen_scale;
        if !known || second_v || seen_kind.is_some_and(|seen| seen != *kind && *kind != 'V') {
            let refusal = Err(refused(pattern));
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                line_no,
                "copybook: refusing a character past X, 9 and V, a second V or a mixed X and 9"
            );
            return refusal;
        }
        if *kind == 'V' {
            seen_scale = true;
            seen_kind = Some('9');
            index = index.saturating_add(1);
            continue;
        }
        seen_kind = Some(*kind);
        index = index.saturating_add(1);
        let mut count: usize = 1;
        if chars.get(index) == Some(&'(') {
            index = index.saturating_add(1);
            let Some(parsed) = parenthesised_count(&chars, &mut index) else {
                let refusal = Err(refused(pattern));
                #[cfg(feature = "trace")]
                crate::trace::debug!(
                    line_no,
                    "copybook: refusing an unterminated, non-numeric or zero picture count"
                );
                return refusal;
            };
            count = parsed;
        }
        if *kind == 'X' {
            if signed || seen_scale {
                let refusal = Err(refused(pattern));
                #[cfg(feature = "trace")]
                crate::trace::debug!(line_no, "copybook: refusing a signed or scaled X picture");
                return refusal;
            }
            let held: usize = match text_len {
                Some(drawn) => drawn.saturating_add(count),
                None => count,
            };
            text_len = Some(held);
        } else {
            digits = digits.saturating_add(count);
            if seen_scale {
                let Ok(add) = u32::try_from(count) else {
                    let refusal = Err(refused(pattern));
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(
                        line_no,
                        "copybook: refusing a picture count past 32 bits"
                    );
                    return refusal;
                };
                scale = scale.saturating_add(add);
            }
        }
    }
    let comp_over_text = usage != Usage::Display && text_len.is_some();
    if comp_over_text || (text_len.is_none() && digits == 0) || digits > 31 {
        let refusal = Err(refused(pattern));
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            line_no,
            "copybook: refusing a COMP picture over text, an empty picture or one past 31 digits"
        );
        return refusal;
    }
    Ok(Picture {
        text_len,
        digits,
        scale,
        signed,
        usage,
    })
}
