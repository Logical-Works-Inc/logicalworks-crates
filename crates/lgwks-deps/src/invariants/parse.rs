//! Building one `[[invariant]]` entry from the shared line reader's raw block,
//! refusing anything the schema cannot represent.

use super::{
    ENFORCEMENT_FIELD, EVIDENCE_FIELDS, EnforcedBy, Enforcement, Entry, ErrorKind, Evidence,
    PASSING_RESULT, REQUIRED_FIELDS, refused, valid_revision,
};
use crate::contract::{self, RawEntry};

/// The text `raw` wrote for `key`, or the empty string for a key it did not
/// write. Every key read through here is one the required-field loop in
/// [`build`] has already proved present, or one whose absence is itself the
/// answer (an optional field), so the empty string is never a guessed value.
fn written(raw: &RawEntry, key: &str) -> String {
    match raw.get(key) {
        Some(value) => value.to_owned(),
        None => String::new(),
    }
}

/// Builds an invariant entry, refusing anything the schema cannot represent.
///
/// Order matters for diagnostics: the closed grammars are parsed before the
/// evidence that depends on them, so an entry that writes both a bad
/// enforcement kind and an unrelated evidence block is told about the
/// enforcement kind.
pub(super) fn build(raw: &RawEntry) -> Result<Entry, ErrorKind> {
    // A block that names no invariant is reported at its own line. The name is
    // kept as an `Option` rather than a placeholder so the refusal can say which
    // field is missing and where the block is, instead of naming an identifier
    // no register wrote.
    let id = raw.get("id").map(str::to_owned);
    for field in REQUIRED_FIELDS {
        // A blank value is a missing value for every free-text field, and for
        // `enforcement` it is a value outside the closed grammar — the repair is
        // different, so the diagnostic has to be.
        let absent = raw.get(field).is_none();
        let blank = raw.get(field).is_some_and(|value| value.trim().is_empty())
            && field != ENFORCEMENT_FIELD;
        if absent || blank {
            return refused(ErrorKind::MissingField {
                id,
                field,
                line: raw.line(),
            });
        }
    }
    // The loop above proved `id` is present and non-blank, so every refusal from
    // here on can name the invariant. The arm below is the reordering guard: a
    // block that reached it without an `id` could not have passed the loop, and
    // reporting that is the direction a parser bug must surface in rather than
    // the direction it must be assumed away in.
    let Some(id) = id else {
        return refused(ErrorKind::MissingField {
            id: None,
            field: "id",
            line: raw.line(),
        });
    };
    let approved_on = written(raw, "approved_on");
    if !contract::is_iso_date(&approved_on) {
        // The required-field loop proved `approved_on` is present, so this is
        // its own line; the block's line is where the repair is if the loop is
        // ever reordered to let a blank value through.
        let line = match raw.field_line("approved_on") {
            Some(line) => line,
            None => raw.line(),
        };
        return refused(ErrorKind::BadDate {
            id,
            line,
            value: approved_on,
        });
    }
    let statement = written(raw, "statement");
    if let Some(reason) = non_monitorable_reason(&statement) {
        return refused(ErrorKind::NonMonitorable { id, reason });
    }
    let authored_kind = written(raw, "enforcement");
    let Some(enforcement) = Enforcement::parse(&authored_kind) else {
        return refused(ErrorKind::UnsupportedEnforcement {
            id,
            enforcement: authored_kind,
        });
    };
    let enforced_by = match raw.get("enforced_by") {
        None => None,
        Some(value) if value.trim().is_empty() => None,
        Some(value) => {
            Some(
                EnforcedBy::parse(value).map_err(|reason| ErrorKind::EnforcerNotConfined {
                    id: id.clone(),
                    spelling: value.to_owned(),
                    reason,
                })?,
            )
        }
    };
    let evidence = build_evidence(raw, &id, enforcement, enforced_by.as_ref())?;
    Ok(Entry {
        id,
        statement,
        owner: written(raw, "owner"),
        scope: written(raw, "scope"),
        enforcement,
        enforced_by,
        evidence,
        approved_by: written(raw, "approved_by"),
        approved_on,
        review: written(raw, "review"),
        line: raw.line(),
    })
}

/// Validates and assembles the optional recorded run.
///
/// The triple is all-or-nothing: a revision with no invocation is a note, not a
/// run, and admitting it would put a revision beside a verdict no execution
/// produced. `enforcer` is the entry's own resolved reference, and a non-empty
/// invocation must name its spelling as a whole token — a passing run of some
/// *other* check is exactly the false comfort this rule exists to refuse.
fn build_evidence(
    raw: &RawEntry,
    id: &str,
    enforcement: Enforcement,
    enforcer: Option<&EnforcedBy>,
) -> Result<Option<Evidence>, ErrorKind> {
    let present: Vec<&str> = EVIDENCE_FIELDS
        .iter()
        .copied()
        .filter(|field| raw.get(field).is_some_and(|value| !value.trim().is_empty()))
        .collect();
    if present.is_empty() {
        if enforcement.requires_evidence() {
            return refused(ErrorKind::MonitorNeedsEvidence { id: id.to_owned() });
        }
        return Ok(None);
    }
    for field in EVIDENCE_FIELDS {
        if !present.contains(&field) {
            return refused(ErrorKind::EvidenceIncomplete {
                id: id.to_owned(),
                missing: field,
            });
        }
    }
    // The loop above proved all three fields present and non-blank.
    let field = |name: &str| written(raw, name).trim().to_owned();
    let revision = field("evidence_revision");
    if !valid_revision(&revision) {
        return refused(ErrorKind::EvidenceRevisionShape {
            id: id.to_owned(),
            revision,
        });
    }
    let result = field("evidence_result");
    if result != PASSING_RESULT {
        return refused(ErrorKind::EvidenceNotPassing {
            id: id.to_owned(),
            result,
        });
    }
    let invocation = field("evidence_invocation");
    if let Some(selector) = enforcer.map(EnforcedBy::selector)
        && !names_selector(&invocation, &selector)
    {
        return refused(ErrorKind::EvidenceDoesNotNameEnforcer {
            id: id.to_owned(),
            invocation,
            selector,
        });
    }
    Ok(Some(Evidence {
        revision,
        invocation,
    }))
}

/// Whether `invocation` names `selector` as a whole token.
///
/// Containing the selector is not naming it: Clippy has lints that are strict
/// prefixes of others (`panic` / `panic_in_result_fn`), and a path is a suffix
/// of a longer path (`vendor/tests/a.rs`) or a prefix of a sibling
/// (`tests/a.rs.orig`). A run of any of those is a run of something else, so an
/// occurrence counts only when neither neighbour could continue a lint name or
/// a path (`sim_invariant_register`, the prefix scenario).
fn names_selector(invocation: &str, selector: &str) -> bool {
    !selector.is_empty()
        && invocation.match_indices(selector).any(|(at, _)| {
            let before = invocation
                .get(..at)
                .and_then(|head| head.chars().next_back());
            let after = invocation
                .get(at.saturating_add(selector.len())..)
                .and_then(|tail| tail.chars().next());
            !before.is_some_and(continues_selector) && !after.is_some_and(continues_selector)
        })
}

/// Whether `character` could extend a lint selector or a repository path.
fn continues_selector(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | '\\' | ':')
}

/// Returns a load-time diagnostic for constraints that cannot be decided from
/// any finite monitor prefix. The register intentionally keeps this language
/// small: modal eventuality and unbounded quantification are documentation,
/// not executable enforcement.
fn non_monitorable_reason(statement: &str) -> Option<String> {
    let compact = statement
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
        .replace('→', "->");
    if compact.contains("g(") && compact.contains("->f") {
        return Some("G(r -> F a) has no finite good or bad prefix".to_owned());
    }
    if compact.contains("forall(")
        || compact.contains("exists(")
        || compact.contains('∀')
        || compact.contains('∃')
    {
        return Some("unbounded quantification is outside the bounded scalar domain".to_owned());
    }
    None
}

/// Maps the shared parser's structural errors into this register's diagnostic
/// namespace without giving invariants a second parser.
pub(super) fn map_contract_error(error: contract::ContractError) -> ErrorKind {
    match error {
        contract::ContractError::Malformed { line, text } => ErrorKind::Malformed { line, text },
        contract::ContractError::UnknownKey { line, key } => ErrorKind::UnknownKey { line, key },
        contract::ContractError::OrphanKey { line, key } => ErrorKind::OrphanKey { line, key },
        // A register block that named no crate keeps its own line: the
        // invariant register reports the dependency register's refusal, and a
        // nameless block is identified by where it is rather than by a name it
        // never wrote.
        contract::ContractError::MissingField { krate, field, line } => ErrorKind::Malformed {
            line,
            text: match krate {
                Some(krate) => format!("{krate:?} is missing required field {field:?}"),
                None => format!("approval block is missing required field {field:?}"),
            },
        },
        contract::ContractError::BadTier { line, value } => ErrorKind::Malformed {
            line,
            text: format!("tier = {value:?}"),
        },
        contract::ContractError::BadDate { krate, line, value } => ErrorKind::BadDate {
            id: krate,
            line,
            value,
        },
        contract::ContractError::ThinReason { krate } => ErrorKind::Malformed {
            line: 0,
            text: format!("reason for {krate:?} is not an invariant field"),
        },
        contract::ContractError::DuplicateEntry { krate, line } => ErrorKind::Malformed {
            line,
            text: format!("duplicate dependency approval {krate:?}"),
        },
        contract::ContractError::BadPolicyValue { line, key, value } => {
            ErrorKind::BadPolicyValue { line, key, value }
        }
        contract::ContractError::DuplicatePolicyKey { line, key } => {
            ErrorKind::DuplicatePolicyKey { line, key }
        }
        contract::ContractError::DuplicatePolicySection { line } => {
            ErrorKind::DuplicatePolicySection { line }
        }
        // These carry their own rendering. The invariant schema has neither a
        // `schema` policy key nor aliases, so the last two cannot arise from an
        // invariant register; they are mapped rather than panicked so a future
        // shared-parser change stays fail-closed.
        error @ (contract::ContractError::InvalidString { line, .. }
        | contract::ContractError::InvalidField { line, .. }
        | contract::ContractError::DuplicateEntryKey {
            duplicate_line: line,
            ..
        }
        | contract::ContractError::UnsupportedSchema { line, .. }
        | contract::ContractError::AliasCollision { line, .. }) => ErrorKind::Malformed {
            line,
            text: error.to_string(),
        },
        // Only `Contract::parse` decodes the repository-policy keys, and this
        // register refuses them before that point; mapped for the same reason.
        error @ contract::ContractError::IncompletePolicy { .. } => ErrorKind::Malformed {
            line: 0,
            text: error.to_string(),
        },
    }
}
