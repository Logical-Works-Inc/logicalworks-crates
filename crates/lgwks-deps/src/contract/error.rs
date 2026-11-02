//! Why a register is not a contract, and how each refusal reads.

use std::error::Error;
use std::fmt;

/// Why a register is not a contract. Each variant carries the line so the
/// message can be pasted straight into an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractError {
    /// A line matched neither a section header, a key/value pair, nor a comment.
    Malformed {
        /// One-based line number.
        line: usize,
        /// The offending line, trimmed.
        text: String,
    },
    /// A key appeared that the schema does not define.
    UnknownKey {
        /// One-based line number.
        line: usize,
        /// The offending key.
        key: String,
    },
    /// A key/value pair appeared before any section header.
    OrphanKey {
        /// One-based line number.
        line: usize,
        /// The offending key.
        key: String,
    },
    /// A required field was absent from an entry.
    MissingField {
        /// The entry's crate name, absent when the block named no crate.
        ///
        /// An `Option` because a block that declares no `crate` has no name to
        /// report: a placeholder would be a name no register ever wrote, and a
        /// reader chasing it would look for a crate the file does not mention.
        /// The block's line identifies it instead.
        krate: Option<String>,
        /// The absent field.
        field: &'static str,
        /// One-based line where the block that lacks the field opened.
        line: usize,
    },
    /// `tier` held a value outside `boundary` / `vendor`.
    BadTier {
        /// One-based line number.
        line: usize,
        /// The offending value.
        value: String,
    },
    /// A `[policy]` value was outside its closed grammar.
    ///
    /// `enforce` admits exactly `true` and exactly `false`. Every other token,
    /// including `True`, `"true"` and `1`, is this refusal: the earlier reader
    /// read any unrecognised spelling as `false`, which let a typo stand the
    /// whole gate down without a diagnostic.
    BadPolicyValue {
        /// One-based line number.
        line: usize,
        /// The offending key.
        key: String,
        /// The offending value, verbatim, quotes included.
        value: String,
    },
    /// A `[policy]` key that only means something beside another was written
    /// without it: `frozen_surfaces` names what is frozen and `frozen_tier` what
    /// it is frozen at, and either alone is half a rule.
    IncompletePolicy {
        /// The key that was written.
        present: &'static str,
        /// The key it needs beside it.
        missing: &'static str,
    },
    /// The same `[policy]` key was written more than once.
    DuplicatePolicyKey {
        /// One-based line number.
        line: usize,
        /// The repeated key.
        key: String,
    },
    /// `[policy]` was declared more than once.
    DuplicatePolicySection {
        /// One-based line number of the second declaration.
        line: usize,
    },
    /// `approved_on` was not an ISO `YYYY-MM-DD` date.
    BadDate {
        /// The entry's crate name.
        krate: String,
        /// One-based line where the field was written.
        line: usize,
        /// The offending value.
        value: String,
    },
    /// A string value is outside the supported TOML basic-string subset.
    InvalidString {
        /// One-based line number.
        line: usize,
        /// The field that carried the value.
        key: String,
        /// The authored value, including quotes.
        value: String,
        /// The syntax rule that was not met.
        reason: &'static str,
    },
    /// A repeated field key makes an entry ambiguous.
    DuplicateEntryKey {
        /// Repeated field name.
        key: String,
        /// Repeated table header, such as `[[approved]]`.
        entry_header: String,
        /// One-based entry ordinal in this register.
        entry_index: usize,
        /// One-based line where the table opened.
        entry_line: usize,
        /// One-based line of the first assignment.
        first_line: usize,
        /// One-based line of the duplicate assignment.
        duplicate_line: usize,
    },
    /// A decoded field value does not satisfy the schema's closed vocabulary.
    InvalidField {
        /// The entry's crate name.
        krate: String,
        /// The field that failed validation.
        field: &'static str,
        /// One-based line where the field was written.
        line: usize,
        /// Decoded offending value.
        value: String,
        /// Accepted value shape.
        expected: &'static str,
    },
    /// `reason` did not name what the standard library cannot do. A reason must
    /// be a sentence (at least four words, at least 24 characters, ending in a
    /// full stop) and must not merely restate the crate's name.
    ThinReason {
        /// The entry's crate name.
        krate: String,
    },
    /// The same crate/capability owner pair was approved twice.
    DuplicateEntry {
        /// The repeated crate name.
        krate: String,
        /// One-based line where the duplicate opened.
        line: usize,
    },
    /// A register schema version this build does not implement.
    UnsupportedSchema {
        /// One-based line of the `schema` assignment.
        line: usize,
        /// The version that was written.
        value: String,
    },
    /// An alias a second identity also claims.
    ///
    /// An alias is a compatibility spelling, so exactly one Cargo package may
    /// own it; two packages sharing one alias would share one authority, which
    /// is the defect the explicit alias exists to avoid.
    AliasCollision {
        /// The contested spelling.
        alias: String,
        /// The package that already owns it.
        owner: String,
        /// The one-based line where the entry claiming it again opened.
        line: usize,
    },
}

/// Renders `ContractError::Malformed`. The offending text is quoted with `{:?}`
/// so trailing whitespace and an empty line are both visible in the message.
pub(super) fn fmt_malformed(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    text: &str,
) -> fmt::Result {
    write!(formatter, "line {line}: cannot parse {text:?}")
}

/// Renders `ContractError::UnknownKey`, naming the key the schema does not
/// define so the register can be corrected without re-reading this module.
pub(super) fn fmt_unknown_key(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    key: &str,
) -> fmt::Result {
    write!(formatter, "line {line}: unknown key {key:?}")
}

/// Renders `ContractError::OrphanKey`, which is what a key/value pair written
/// above the first section header produces.
pub(super) fn fmt_orphan_key(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    key: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: key {key:?} appears before any section header"
    )
}

/// Renders `ContractError::MissingField`, always at the line the block opened.
///
/// The block's own line is the position a reader can act from even when the block
/// never named itself, so a nameless block is reported by where it is rather than
/// under a name no register wrote.
pub(super) fn fmt_missing_field(
    formatter: &mut fmt::Formatter<'_>,
    krate: Option<&str>,
    field: &str,
    line: usize,
) -> fmt::Result {
    match krate {
        Some(krate) => write!(
            formatter,
            "line {line}: approval for {krate:?} is missing required field {field:?}"
        ),
        None => write!(
            formatter,
            "line {line}: approval block is missing required field {field:?}"
        ),
    }
}

/// Renders `ContractError::BadTier`, restating the only two admissible values
/// so the message is actionable on its own.
pub(super) fn fmt_bad_tier(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    value: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: tier {value:?} is not 'boundary' or 'vendor'"
    )
}

/// Renders `ContractError::BadPolicyValue`. A quoted `"true"` is shown with its
/// quotes so the reader can see that the register carried a string where the
/// grammar requires a Boolean.
pub(super) fn fmt_bad_policy_value(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    key: &str,
    value: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: [policy] {key} = {value:?} is not a Boolean; write exactly true or false"
    )
}

/// Renders `ContractError::DuplicatePolicyKey`, naming the repeated key so the
/// second assignment can be deleted without re-reading this module.
pub(super) fn fmt_duplicate_policy_key(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    key: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: [policy] {key} is written more than once; keep one assignment"
    )
}

/// Renders `ContractError::DuplicatePolicySection` at the second `[policy]`, so
/// the two declarations can be merged rather than guessed between.
pub(super) fn fmt_duplicate_policy_section(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: [policy] is declared more than once; a second declaration would \
         silently override the first"
    )
}

/// Renders `ContractError::BadDate` at the field's source location.
pub(super) fn fmt_bad_date(
    formatter: &mut fmt::Formatter<'_>,
    krate: &str,
    line: usize,
    value: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: approval for {krate:?} has invalid approved_on {value:?}; want a real YYYY-MM-DD date"
    )
}

/// Renders a strict-string refusal at its source line and field.
pub(super) fn fmt_invalid_string(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    key: &str,
    value: &str,
    reason: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: {key} value {value:?} is not supported TOML: {reason}"
    )
}

/// Renders both locations and the repeated table identity for an ambiguous key.
pub(super) fn fmt_duplicate_entry_key(
    formatter: &mut fmt::Formatter<'_>,
    key: &str,
    entry_header: &str,
    entry_index: usize,
    entry_line: usize,
    first_line: usize,
    duplicate_line: usize,
) -> fmt::Result {
    write!(
        formatter,
        "line {duplicate_line}: duplicate key {key:?} in {entry_header} entry #{entry_index} opened at line {entry_line}; first assignment is at line {first_line}"
    )
}

/// Renders an offending decoded value with its field and source line.
pub(super) fn fmt_invalid_field(
    formatter: &mut fmt::Formatter<'_>,
    krate: &str,
    field: &str,
    line: usize,
    value: &str,
    expected: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: approval for {krate:?} has invalid {field} value {value:?}; {expected}"
    )
}

/// Renders `ContractError::ThinReason`, spelling out the sentence rule so the
/// refusal explains itself rather than pointing back at the source.
pub(super) fn fmt_thin_reason(formatter: &mut fmt::Formatter<'_>, krate: &str) -> fmt::Result {
    write!(
        formatter,
        "approval for {krate:?} needs a reason naming what std cannot do — \
         a sentence of four or more words ending in a full stop"
    )
}

/// Renders `ContractError::DuplicateEntry` at the line the second approval
/// opened, not at the line of the first.
pub(super) fn fmt_duplicate(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    krate: &str,
) -> fmt::Result {
    write!(formatter, "line {line}: {krate:?} is already approved")
}

/// Renders `ContractError::UnsupportedSchema`, naming the versions this build
/// can read so the repair is "migrate or downgrade", not "guess".
pub(super) fn fmt_unsupported_schema(
    formatter: &mut fmt::Formatter<'_>,
    line: usize,
    value: &str,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: register schema {value:?} is not supported; this build reads schema 1, 2 and 3"
    )
}

/// Renders `ContractError::AliasCollision`, naming both the contested spelling
/// and the package that already owns it.
pub(super) fn fmt_alias_collision(
    formatter: &mut fmt::Formatter<'_>,
    alias: &str,
    owner: &str,
    line: usize,
) -> fmt::Result {
    write!(
        formatter,
        "line {line}: alias {alias:?} is already claimed by {owner:?}; a compatibility alias names exactly one Cargo package"
    )
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `ContractError` is not `Copy`, so the borrowed payloads are bound by
        // `ref`; matching the dereferenced value keeps the pattern types equal
        // to the variant types without cloning a single `String`.
        match *self {
            Self::Malformed { line, ref text } => fmt_malformed(formatter, line, text),
            Self::UnknownKey { line, ref key } => fmt_unknown_key(formatter, line, key),
            Self::OrphanKey { line, ref key } => fmt_orphan_key(formatter, line, key),
            Self::MissingField {
                ref krate,
                field,
                line,
            } => fmt_missing_field(formatter, krate.as_deref(), field, line),
            Self::BadTier { line, ref value } => fmt_bad_tier(formatter, line, value),
            Self::BadPolicyValue {
                line,
                ref key,
                ref value,
            } => fmt_bad_policy_value(formatter, line, key, value),
            Self::DuplicatePolicyKey { line, ref key } => {
                fmt_duplicate_policy_key(formatter, line, key)
            }
            Self::DuplicatePolicySection { line } => fmt_duplicate_policy_section(formatter, line),
            Self::IncompletePolicy { present, missing } => write!(
                formatter,
                "[policy] declares `{present}` without `{missing}`; the two keys are one rule \
                 and are written together"
            ),
            Self::BadDate {
                ref krate,
                line,
                ref value,
            } => fmt_bad_date(formatter, krate, line, value),
            Self::InvalidString {
                line,
                ref key,
                ref value,
                reason,
            } => fmt_invalid_string(formatter, line, key, value, reason),
            Self::DuplicateEntryKey {
                ref key,
                ref entry_header,
                entry_index,
                entry_line,
                first_line,
                duplicate_line,
            } => fmt_duplicate_entry_key(
                formatter,
                key,
                entry_header,
                entry_index,
                entry_line,
                first_line,
                duplicate_line,
            ),
            Self::InvalidField {
                ref krate,
                field,
                line,
                ref value,
                expected,
            } => fmt_invalid_field(formatter, krate, field, line, value, expected),
            Self::ThinReason { ref krate } => fmt_thin_reason(formatter, krate),
            Self::DuplicateEntry { line, ref krate } => fmt_duplicate(formatter, line, krate),
            Self::UnsupportedSchema { line, ref value } => {
                fmt_unsupported_schema(formatter, line, value)
            }
            Self::AliasCollision {
                ref alias,
                ref owner,
                line,
            } => fmt_alias_collision(formatter, alias, owner, line),
        }
    }
}

impl Error for ContractError {}
