//! `retire_read` owns the schema-3 register reader: the `[approved.retire]`
//! subtable, the cost inline table, and the validation that turns one entry's
//! mortgage into a [`Retire`](crate::retire::Retire) or a typed refusal.
//!
//! The reader half of [`crate::retire`]: parsing lives here so the audit and
//! the report stay a separate responsibility (#788).

use crate::{
    contract::{ContractError, RawEntry, decode_string, is_iso_date},
    lifecycle::{field_line, invalid, is_mortgage_sentence},
    retire::{Retire, RetireCost},
};

// ── Reader ──────────────────────────────────────────────────────────────────

/// The retire subtable's keys, each stored under its `retire.` prefixed name.
const RETIRE_KEYS: [(&str, &str); 4] = [
    ("replacement", "retire.replacement"),
    ("blocker", "retire.blocker"),
    ("cost", "retire.cost"),
    ("review", "retire.review"),
];

/// Admits one `key = value` pair written inside `[approved.retire]`.
pub(crate) fn apply_pair(
    key: &str,
    value: &str,
    line_no: usize,
    draft: &mut RawEntry,
) -> Result<(), ContractError> {
    let Some(stored) = RETIRE_KEYS
        .iter()
        .find(|candidate| candidate.0 == key)
        .map(|candidate| candidate.1)
    else {
        let refusal = Err(ContractError::UnknownKey {
            line: line_no,
            key: key.to_owned(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::apply_pair: returning an error to the caller");
        return refusal;
    };
    if let Some(previous) = draft.fields.iter().find(|field| field.key == stored) {
        let refusal = Err(ContractError::DuplicateEntryKey {
            key: key.to_owned(),
            entry_header: "[approved.retire]".to_owned(),
            entry_index: draft.index,
            entry_line: draft.line(),
            first_line: previous.line,
            duplicate_line: line_no,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::apply_pair: returning an error to the caller");
        return refusal;
    }
    let decoded = if key == "cost" {
        value.to_owned()
    } else {
        decode_string(value, key, line_no)?
    };
    draft.fields.push(crate::contract::RawField {
        key: stored,
        value: decoded,
        line: line_no,
    });
    Ok(())
}

/// Splits one inline `{ k = v, ... }` table on its top-level commas.
fn split_inline(body: &str) -> Option<Vec<String>> {
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0_usize;
    for (index, byte) in body.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b'{' if !quoted => depth = depth.saturating_add(1),
            b'}' if !quoted => depth = depth.saturating_sub(1),
            b',' if !quoted && depth == 0 => {
                parts.push(body[start..index].to_owned());
                start = index.saturating_add(1);
            }
            _ => {}
        }
    }
    parts.push(body[start..].to_owned());
    Some(parts)
}

/// Reads one `name = value` pair of the cost inline table.
fn cost_field(pair: &str) -> Option<(&str, &str)> {
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some((name, value.trim()))
}

/// Parses the `cost` inline table into its five measured fields.
fn parse_cost(
    value: &str,
    line: usize,
    krate: &str,
    draft: &RawEntry,
) -> Result<RetireCost, ContractError> {
    let body = value
        .trim()
        .strip_prefix('{')
        .and_then(|inner| inner.strip_suffix('}'));
    let Some(body) = body else {
        let refusal = Err(invalid(
            draft,
            krate,
            "retire.cost",
            value.to_owned(),
            "expected an inline table such as { cold_build_s = 1.2, crates = 3, binary_bytes = 4, measured = \"2026-11-02\", command = \"cargo build --locked -p lgwks_deps\" }",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    let Some(pairs) = split_inline(body) else {
        let refusal = Err(invalid(
            draft,
            krate,
            "retire.cost",
            value.to_owned(),
            "expected an inline table with five comma-separated fields",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    let mut cold_build_s: Option<f64> = None;
    let mut crates: Option<u64> = None;
    let mut binary_bytes: Option<u64> = None;
    let mut measured: Option<String> = None;
    let mut command: Option<String> = None;
    for pair in &pairs {
        let Some((name, raw)) = cost_field(pair) else {
            let refusal = Err(invalid(
                draft,
                krate,
                "retire.cost",
                pair.clone(),
                "expected a name = value pair inside the cost table",
            ));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
            return refusal;
        };
        match name {
            "cold_build_s" => {
                let parsed = raw.parse::<f64>().ok().filter(|parsed| parsed.is_finite());
                let Some(parsed) = parsed else {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.cold_build_s",
                        raw.to_owned(),
                        "expected a finite number of seconds",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                };
                cold_build_s = Some(parsed);
            }
            "crates" => {
                let Some(parsed) = raw.parse::<u64>().ok() else {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.crates",
                        raw.to_owned(),
                        "expected a whole number of crates",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                };
                crates = Some(parsed);
            }
            "binary_bytes" => {
                let Some(parsed) = raw.parse::<u64>().ok() else {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.binary_bytes",
                        raw.to_owned(),
                        "expected a whole number of bytes",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                };
                binary_bytes = Some(parsed);
            }
            "measured" => {
                let Ok(decoded) = decode_string(raw, "retire.cost.measured", line) else {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.measured",
                        raw.to_owned(),
                        "expected a double-quoted date string",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                };
                if !is_iso_date(&decoded) {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.measured",
                        decoded,
                        "expected a real YYYY-MM-DD date",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                }
                measured = Some(decoded);
            }
            "command" => {
                let Ok(decoded) = decode_string(raw, "retire.cost.command", line) else {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.command",
                        raw.to_owned(),
                        "expected a double-quoted command string",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                };
                if decoded.trim().is_empty() {
                    let refusal = Err(invalid(
                        draft,
                        krate,
                        "retire.cost.command",
                        decoded,
                        "expected the non-empty command that measured this cost",
                    ));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                    return refusal;
                }
                command = Some(decoded);
            }
            _ => {
                let refusal = Err(invalid(
                    draft,
                    krate,
                    "retire.cost",
                    name.to_owned(),
                    "expected one of cold_build_s, crates, binary_bytes, measured, command",
                ));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
                return refusal;
            }
        }
    }
    let Some(cold_build_s) = cold_build_s else {
        let refusal = Err(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field: "retire.cost.cold_build_s",
            line: field_line(draft, "retire.cost"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    let Some(crates) = crates else {
        let refusal = Err(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field: "retire.cost.crates",
            line: field_line(draft, "retire.cost"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    let Some(binary_bytes) = binary_bytes else {
        let refusal = Err(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field: "retire.cost.binary_bytes",
            line: field_line(draft, "retire.cost"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    let Some(measured) = measured else {
        let refusal = Err(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field: "retire.cost.measured",
            line: field_line(draft, "retire.cost"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    let Some(command) = command else {
        let refusal = Err(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field: "retire.cost.command",
            line: field_line(draft, "retire.cost"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::parse_cost: returning an error to the caller");
        return refusal;
    };
    Ok(RetireCost {
        cold_build_s,
        crates,
        binary_bytes,
        measured,
        command,
    })
}

/// Requires a retire field to be present and non-blank.
fn require_retire(
    draft: &RawEntry,
    krate: &str,
    field: &'static str,
) -> Result<String, ContractError> {
    match draft.get(field) {
        Some(value) if !value.trim().is_empty() => Ok(value.to_owned()),
        _ => {
            let refusal = Err(ContractError::MissingField {
                krate: Some(krate.to_owned()),
                field,
                line: draft.line(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::require_retire: returning an error to the caller");
            refusal
        }
    }
}

/// Turns one entry's `[approved.retire]` table into its mortgage, or `None`
/// when the entry carries no retire table at all.
///
/// Presence is partial-table aware: one retire key means the author started
/// the mortgage, and every absent key is then a refusal naming it, not a
/// silent fallback to an unmortgaged edge.
pub(crate) fn build(draft: &RawEntry, krate: &str) -> Result<Option<Retire>, ContractError> {
    if RETIRE_KEYS
        .iter()
        .all(|candidate| draft.get(candidate.1).is_none())
    {
        return Ok(None);
    }
    let replacement = require_retire(draft, krate, "retire.replacement")?;
    if !crate::retire::is_replacement_shape(&replacement) {
        let refusal = Err(invalid(
            draft,
            krate,
            "retire.replacement",
            replacement,
            "expected a module path such as lgwks_std::json",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::build: returning an error to the caller");
        return refusal;
    }
    let blocker = require_retire(draft, krate, "retire.blocker")?;
    if !is_mortgage_sentence(&blocker) {
        let refusal = Err(invalid(
            draft,
            krate,
            "retire.blocker",
            blocker,
            "expected a sentence of four or more words ending in a full stop",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::build: returning an error to the caller");
        return refusal;
    }
    let cost = match draft.get("retire.cost") {
        Some(value) => parse_cost(value, field_line(draft, "retire.cost"), krate, draft)?,
        None => {
            let refusal = Err(ContractError::MissingField {
                krate: Some(krate.to_owned()),
                field: "retire.cost",
                line: draft.line(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::build: returning an error to the caller");
            return refusal;
        }
    };
    let review = require_retire(draft, krate, "retire.review")?;
    if !is_iso_date(&review) {
        let refusal = Err(invalid(
            draft,
            krate,
            "retire.review",
            review,
            "expected a real YYYY-MM-DD date",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read::build: returning an error to the caller");
        return refusal;
    }
    Ok(Some(Retire {
        replacement,
        blocker,
        cost,
        review,
    }))
}
