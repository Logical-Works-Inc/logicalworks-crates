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

/// `error` as an `Err`, with the refusal recorded on the debug stream.
fn refused<T>(error: ContractError) -> Result<T, ContractError> {
    let refusal = Err(error);
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire_read: returning an error to the caller");
    refusal
}

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
        return refused(ContractError::UnknownKey {
            line: line_no,
            key: key.to_owned(),
        });
    };
    if let Some(previous) = draft.fields.iter().find(|field| field.key == stored) {
        return refused(ContractError::DuplicateEntryKey {
            key: key.to_owned(),
            entry_header: "[approved.retire]".to_owned(),
            entry_index: draft.index,
            entry_line: draft.line(),
            first_line: previous.line,
            duplicate_line: line_no,
        });
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
fn split_inline(body: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for ch in body.chars() {
        if escaped {
            escaped = false;
            current.push(ch);
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '{' if !quoted => depth = depth.saturating_add(1),
            '}' if !quoted => depth = depth.saturating_sub(1),
            ',' if !quoted && depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    parts.push(current);
    parts
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

/// The five measured cost fields as read so far; each is still optional.
#[derive(Default)]
struct CostFields {
    /// Seconds a cold build took.
    cold_build_s: Option<f64>,
    /// Crates compiled.
    crates: Option<u64>,
    /// Size of the built binary.
    binary_bytes: Option<u64>,
    /// ISO date of the measurement.
    measured: Option<String>,
    /// Command that measured it.
    command: Option<String>,
}

/// What a cost table is read against: the entry it belongs to and where it was
/// written, so every refusal names both.
struct CostReader<'a> {
    /// The entry being read.
    draft: &'a RawEntry,
    /// The crate the entry approves.
    krate: &'a str,
    /// The line the `cost` key was written on.
    line: usize,
}

impl CostReader<'_> {
    /// A refusal naming `field`, the offending `value`, and what was expected.
    fn invalid<T>(
        &self,
        field: &'static str,
        value: String,
        expected: &'static str,
    ) -> Result<T, ContractError> {
        refused(invalid(self.draft, self.krate, field, value, expected))
    }

    /// A refusal for a cost field the table never wrote.
    fn missing<T>(&self, field: &'static str) -> Result<T, ContractError> {
        refused(ContractError::MissingField {
            krate: Some(self.krate.to_owned()),
            field,
            line: field_line(self.draft, "retire.cost"),
        })
    }

    /// Reads one `name = raw` pair into `fields`.
    fn read(&self, fields: &mut CostFields, name: &str, raw: &str) -> Result<(), ContractError> {
        match name {
            "cold_build_s" => fields.cold_build_s = Some(self.seconds(raw)?),
            "crates" => {
                let expected = "expected a whole number of crates";
                fields.crates = Some(self.whole("retire.cost.crates", raw, expected)?);
            }
            "binary_bytes" => {
                let expected = "expected a whole number of bytes";
                fields.binary_bytes =
                    Some(self.whole("retire.cost.binary_bytes", raw, expected)?);
            }
            "measured" => fields.measured = Some(self.measured(raw)?),
            "command" => fields.command = Some(self.command(raw)?),
            _ => {
                return self.invalid(
                    "retire.cost",
                    name.to_owned(),
                    "expected one of cold_build_s, crates, binary_bytes, measured, command",
                );
            }
        }
        Ok(())
    }

    /// A finite number of seconds.
    fn seconds(&self, raw: &str) -> Result<f64, ContractError> {
        match raw.parse::<f64>() {
            Ok(parsed) if parsed.is_finite() => Ok(parsed),
            Ok(_) | Err(_) => self.invalid(
                "retire.cost.cold_build_s",
                raw.to_owned(),
                "expected a finite number of seconds",
            ),
        }
    }

    /// A whole number.
    fn whole(
        &self,
        field: &'static str,
        raw: &str,
        expected: &'static str,
    ) -> Result<u64, ContractError> {
        match raw.parse::<u64>() {
            Ok(parsed) => Ok(parsed),
            Err(_) => self.invalid(field, raw.to_owned(), expected),
        }
    }

    /// A double-quoted ISO date.
    fn measured(&self, raw: &str) -> Result<String, ContractError> {
        let Ok(decoded) = decode_string(raw, "retire.cost.measured", self.line) else {
            return self.invalid(
                "retire.cost.measured",
                raw.to_owned(),
                "expected a double-quoted date string",
            );
        };
        if !is_iso_date(&decoded) {
            return self.invalid(
                "retire.cost.measured",
                decoded,
                "expected a real YYYY-MM-DD date",
            );
        }
        Ok(decoded)
    }

    /// A double-quoted, non-empty command.
    fn command(&self, raw: &str) -> Result<String, ContractError> {
        let Ok(decoded) = decode_string(raw, "retire.cost.command", self.line) else {
            return self.invalid(
                "retire.cost.command",
                raw.to_owned(),
                "expected a double-quoted command string",
            );
        };
        if decoded.trim().is_empty() {
            return self.invalid(
                "retire.cost.command",
                decoded,
                "expected the non-empty command that measured this cost",
            );
        }
        Ok(decoded)
    }

    /// The cost, once every one of the five fields was written.
    fn finish(&self, fields: CostFields) -> Result<RetireCost, ContractError> {
        let Some(cold_build_s) = fields.cold_build_s else {
            return self.missing("retire.cost.cold_build_s");
        };
        let Some(crates) = fields.crates else {
            return self.missing("retire.cost.crates");
        };
        let Some(binary_bytes) = fields.binary_bytes else {
            return self.missing("retire.cost.binary_bytes");
        };
        let Some(measured) = fields.measured else {
            return self.missing("retire.cost.measured");
        };
        let Some(command) = fields.command else {
            return self.missing("retire.cost.command");
        };
        Ok(RetireCost {
            cold_build_s,
            crates,
            binary_bytes,
            measured,
            command,
        })
    }
}

/// Parses the `cost` inline table into its five measured fields.
fn parse_cost(
    value: &str,
    line: usize,
    krate: &str,
    draft: &RawEntry,
) -> Result<RetireCost, ContractError> {
    let reader = CostReader { draft, krate, line };
    let body = value
        .trim()
        .strip_prefix('{')
        .and_then(|inner| inner.strip_suffix('}'));
    let Some(body) = body else {
        return reader.invalid(
            "retire.cost",
            value.to_owned(),
            "expected an inline table such as { cold_build_s = 1.2, crates = 3, binary_bytes = 4, measured = \"2026-11-02\", command = \"cargo build --locked -p lgwks_deps\" }",
        );
    };
    let mut fields = CostFields::default();
    for pair in &split_inline(body) {
        let Some((name, raw)) = cost_field(pair) else {
            return reader.invalid(
                "retire.cost",
                pair.clone(),
                "expected a name = value pair inside the cost table",
            );
        };
        reader.read(&mut fields, name, raw)?;
    }
    reader.finish(fields)
}

/// Requires a retire field to be present and non-blank.
fn require_retire(
    draft: &RawEntry,
    krate: &str,
    field: &'static str,
) -> Result<String, ContractError> {
    match draft.get(field) {
        Some(value) if !value.trim().is_empty() => Ok(value.to_owned()),
        _ => refused(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field,
            line: draft.line(),
        }),
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
        return refused(invalid(
            draft,
            krate,
            "retire.replacement",
            replacement,
            "expected a module path such as lgwks_std::json",
        ));
    }
    let blocker = require_retire(draft, krate, "retire.blocker")?;
    if !is_mortgage_sentence(&blocker) {
        return refused(invalid(
            draft,
            krate,
            "retire.blocker",
            blocker,
            "expected a sentence of four or more words ending in a full stop",
        ));
    }
    let Some(cost_text) = draft.get("retire.cost") else {
        return refused(ContractError::MissingField {
            krate: Some(krate.to_owned()),
            field: "retire.cost",
            line: draft.line(),
        });
    };
    let cost = parse_cost(cost_text, field_line(draft, "retire.cost"), krate, draft)?;
    let review = require_retire(draft, krate, "retire.review")?;
    if !is_iso_date(&review) {
        return refused(invalid(
            draft,
            krate,
            "retire.review",
            review,
            "expected a real YYYY-MM-DD date",
        ));
    }
    Ok(Some(Retire {
        replacement,
        blocker,
        cost,
        review,
    }))
}
