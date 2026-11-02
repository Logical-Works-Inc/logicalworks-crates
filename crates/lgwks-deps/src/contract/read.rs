//! The line reader: sections, key pairs and the register's raw blocks, refused
//! line by line rather than skipped.

use super::build::{split_pair, strip_comment};
use super::{ContractError, Entry};

/// Every key an `[[approved]]` block must carry, in the order the parser checks
/// them. The order is observable: `validate_required_fields` reports the first
/// absent key, so a register missing several is told about the earliest one and
/// the message is stable across runs.
pub(super) const REQUIRED: [&str; 13] = [
    "crate",
    "tier",
    "version",
    "owner",
    "capability",
    "license",
    "source",
    "allowed_consumers",
    "allowed_kinds",
    "reason",
    "approved_by",
    "approved_on",
    "review",
];

/// Every key an `[[approved]]` block may carry: the required set plus the
/// optional exact-origin identity and the optional admitted-capability policy.
/// Keeping it separate from `REQUIRED` is what lets an existing class-only,
/// policy-free block keep parsing while a block that writes `origin`,
/// `features` or `target` has it validated and compared.
pub(super) const ENTRY_KEYS: [&str; 20] = [
    "crate",
    "tier",
    "version",
    "owner",
    "capability",
    "license",
    "source",
    "origin",
    "features",
    "required_features",
    "uses_default_features",
    "optional",
    "target",
    "aliases",
    "allowed_consumers",
    "allowed_kinds",
    "reason",
    "approved_by",
    "approved_on",
    "review",
];

/// One repeated register block as it is being read, before validation.
///
/// Fields are kept in source order with their locations, so duplicate
/// diagnostics can name both assignments without adding a TOML or map crate.
#[derive(Default)]
pub(crate) struct RawEntry {
    /// One-based line where the `[[approved]]` header opened. Diagnostics for
    /// the whole block point here, since individual fields carry no lines.
    pub(crate) line: usize,
    /// One-based ordinal among repeated tables in this register.
    pub(crate) index: usize,
    /// Decoded key/value pairs and their source lines.
    pub(crate) fields: Vec<RawField>,
    /// Header that identifies this register's entry type.
    pub(crate) entry_header: String,
}

/// One schema-approved field with decoded value and source location.
pub(crate) struct RawField {
    /// Key selected from the register-specific schema.
    pub(crate) key: &'static str,
    /// Decoded TOML basic-string value.
    pub(crate) value: String,
    /// One-based source line.
    pub(crate) line: usize,
}

impl RawEntry {
    /// Returns the value written for `key`, or `None` if the block never
    /// carried it. An empty string is returned as `Some("")`: presence and
    /// non-emptiness are separate questions, and `check_field_present` is what
    /// rejects the blank case.
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| field.value.as_str())
    }

    /// Returns a field's source line after successful syntax admission.
    pub(crate) fn field_line(&self, key: &str) -> Option<usize> {
        self.fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| field.line)
    }

    /// Returns a field that `validate_required_fields` has already proven
    /// present, or the typed `MissingField` refusal for its absence.
    ///
    /// The absent branch is unreachable from `build`, which validates first;
    /// the method exists so the invariant is carried by the return type rather
    /// than by an `expect` that would abort the process on a parser bug.
    pub(crate) fn require(
        &self,
        key: &'static str,
        krate: Option<&str>,
    ) -> Result<&str, ContractError> {
        self.get(key).ok_or_else(|| ContractError::MissingField {
            krate: krate.map(str::to_owned),
            field: key,
            line: self.line(),
        })
    }

    /// One-based line where this entry opened.
    pub(crate) const fn line(&self) -> usize {
        self.line
    }
}

/// Which part of the register the reader is currently inside, so a key can be
/// dispatched to the policy block, to the open approval block, or to that
/// approval's retire mortgage.
#[derive(PartialEq, Eq)]
pub(super) enum Section {
    /// Before any header. A key here is an orphan and is refused.
    None,
    /// Inside `[policy]`.
    Policy,
    /// Inside the register's repeated entry block, which the reader has
    /// already pushed a [`RawEntry`] for.
    Entry,
    /// Inside one entry's `[approved.retire]` mortgage table (schema 3, #788).
    Retire,
}

/// Mutable state for one pass through either register schema.
pub(super) struct Reader<'a> {
    /// Repeated table header accepted by this register.
    entry_header: &'a str,
    /// Entry keys accepted by this register.
    allowed_keys: &'a [&'static str],
    /// Current section.
    section: Section,
    /// Whether a `[policy]` header has already been read.
    policy_declared: bool,
    /// Policy keys already assigned, so a repeat is a refusal.
    policy_keys: Vec<String>,
    /// Policy enforcement flag, defaulting to true.
    enforce: bool,
    /// Optional repository authority.
    repository: Option<String>,
    /// Register schema version, defaulting to 1 when `[policy] schema` is absent.
    schema: u32,
    /// Repository-policy declarations, decoded later by the register that owns them.
    policy_fields: Vec<RawField>,
    /// Raw entries in source order.
    entries: Vec<RawEntry>,
}

/// Recognises a section header, opening a new raw entry for `entry_header`.
///
/// Returns `Ok(true)` when the line was a header the reader consumed, `Ok(false)`
/// when it is an ordinary key/value line, and `Malformed` for a bracketed line
/// that names no known section: a typo like `[[aproved]]` must not silently
/// fall through and become an unenforced entry.
pub(super) fn handle_section_header(
    line: &str,
    line_no: usize,
    entry_header: &str,
    section: &mut Section,
    policy_declared: &mut bool,
    drafts: &mut Vec<RawEntry>,
) -> Result<bool, ContractError> {
    if line == "[policy]" {
        // A second `[policy]` is refused rather than merged. The two blocks are
        // read by the same reader, so a later one silently overwrites whatever
        // the earlier one set; refusing is what keeps the register's effective
        // enforcement flag the one a reviewer read.
        if *policy_declared {
            let refusal = Err(ContractError::DuplicatePolicySection { line: line_no });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "handle_section_header: returning an error to the caller");
            return refusal;
        }
        *policy_declared = true;
        *section = Section::Policy;
        Ok(true)
    } else if line == entry_header {
        *section = Section::Entry;
        drafts.push(RawEntry {
            line: line_no,
            index: drafts.len().saturating_add(1),
            fields: Vec::new(),
            entry_header: entry_header.to_owned(),
        });
        Ok(true)
    } else if line == "[approved.retire]" && entry_header == "[[approved]]" {
        // A retire table outside an approval is a mortgage with no edge: the
        // section only ever follows an `[[approved]]` header that pushed a
        // draft, so anything else is a header no register shape defines.
        if *section != Section::Entry {
            let refusal = Err(ContractError::Malformed {
                line: line_no,
                text: line.to_owned(),
            });
            lgwks_std::trace::debug!(
                line = line_no,
                "contract::read: a retire table follows no approval"
            );
            return refusal;
        }
        *section = Section::Retire;
        Ok(true)
    } else if line.starts_with('[') {
        Err(ContractError::Malformed {
            line: line_no,
            text: line.to_owned(),
        })
    } else {
        Ok(false)
    }
}

/// Every scalar key the `[policy]` block defines.
pub(super) const POLICY_KEYS: [&str; 3] = ["enforce", "repository", "schema"];

/// The repository-policy keys a dependency register may declare in `[policy]`.
///
/// Read as strings and decoded by [`Contract::parse`], because what they mean
/// is the dependency register's: the invariant register shares the line reader
/// and refuses every one of them.
pub(super) const POLICY_DECLARATION_KEYS: [&str; 4] = [
    "accepted_licenses",
    "surfaces",
    "frozen_surfaces",
    "frozen_tier",
];

/// The register schema versions this build reads.
///
/// `1` is a register that authorises a source class and nothing finer; it is
/// still read so an older committed register does not fail to parse. `2` adds
/// the exact-origin and admitted-capability-policy keys and the explicit alias
/// list. An unknown future version is refused rather than read as either.
pub(super) const SUPPORTED_SCHEMAS: [u32; 3] = [1, 2, 3];

/// Decodes `[policy] enforce` under a closed grammar: exactly `true` or exactly
/// `false`, and nothing else.
///
/// The token is the raw pair value after comment stripping and trimming, so a
/// quoted `"true"` still arrives with its quotes and is refused as a string
/// where the grammar requires a Boolean. This is the difference the previous
/// reader could not make: it read *any* unrecognised spelling as `false`, so
/// `enforce = True`, `enforce = "true"` and `enforce = 1` each stood the whole
/// gate down without a diagnostic. A gate a typo can disable is a gate a typo
/// does disable.
pub(super) fn decode_enforce(value: &str, line_no: usize) -> Result<bool, ContractError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ContractError::BadPolicyValue {
            line: line_no,
            key: "enforce".to_owned(),
            value: value.to_owned(),
        }),
    }
}

/// Decodes `[policy] schema` as an unsigned integer this build implements.
///
/// The token is read bare (an integer, not a string). A version this build does
/// not implement is [`ContractError::UnsupportedSchema`] rather than a default,
/// so a register written for a later schema cannot be read under the older
/// rules and silently lose the keys that schema added.
pub(super) fn decode_schema(value: &str, line_no: usize) -> Result<u32, ContractError> {
    match value.parse::<u32>() {
        Ok(version) if SUPPORTED_SCHEMAS.contains(&version) => Ok(version),
        Ok(version) => {
            lgwks_std::trace::debug!(
                line = line_no,
                version,
                "contract::read: a schema this build does not implement"
            );
            Err(ContractError::UnsupportedSchema {
                line: line_no,
                value: value.to_owned(),
            })
        }
        Err(cause) => {
            lgwks_std::trace::debug!(line = line_no, %cause, "contract::read: a schema that is not an unsigned integer");
            Err(ContractError::UnsupportedSchema {
                line: line_no,
                value: value.to_owned(),
            })
        }
    }
}

/// Decodes a single-line TOML v1.0 basic string used by either register schema.
///
/// Every TOML escape is recognised, but a register value may not contain a
/// control character, raw or decoded: `\b`, `\t`, `\n`, `\f`, `\r` and any
/// `\u`/`\U` escape naming a control character are refused with their own
/// reason rather than as unknown escapes. What a value can carry is therefore
/// printable text plus `\"`, `\\` and non-control Unicode escapes. This is
/// stricter than TOML, which admits a raw tab, and it fails closed. Literal and
/// multiline strings are outside this register subset and refused.
pub(crate) fn decode_string(value: &str, key: &str, line: usize) -> Result<String, ContractError> {
    let invalid = |reason| ContractError::InvalidString {
        line,
        key: key.to_owned(),
        value: value.to_owned(),
        reason,
    };
    let Some(body) = value.strip_prefix('"') else {
        let refusal = Err(invalid("expected a double-quoted basic string"));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
        return refusal;
    };
    let mut decoded = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            if chars.peek().is_some() {
                let refusal = Err(invalid("trailing tokens after the closing quote"));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
                return refusal;
            }
            if decoded.chars().any(char::is_control) {
                let refusal = Err(invalid("decoded control characters are not supported"));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
                return refusal;
            }
            return Ok(decoded);
        }
        if ch.is_control() {
            let refusal = Err(invalid("control characters are not supported"));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
            return refusal;
        }
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            let refusal = Err(invalid("incomplete escape sequence"));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
            return refusal;
        };
        match escaped {
            'b' => decoded.push('\u{0008}'),
            't' => decoded.push('\t'),
            'n' => decoded.push('\n'),
            'f' => decoded.push('\u{000c}'),
            'r' => decoded.push('\r'),
            '"' => decoded.push('"'),
            '\\' => decoded.push('\\'),
            'u' | 'U' => {
                let digits = if escaped == 'u' { 4 } else { 8 };
                let mut scalar = 0_u32;
                for _ in 0..digits {
                    let Some(digit) = chars.next() else {
                        let refusal = Err(invalid("incomplete Unicode escape"));
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
                        return refusal;
                    };
                    let Some(hex) = digit.to_digit(16) else {
                        let refusal = Err(invalid("invalid hexadecimal Unicode escape"));
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
                        return refusal;
                    };
                    scalar = scalar
                        .checked_mul(16)
                        .and_then(|value| value.checked_add(hex))
                        .ok_or_else(|| invalid("Unicode escape is outside the scalar range"))?;
                }
                let Some(decoded_char) = char::from_u32(scalar) else {
                    let refusal = Err(invalid("Unicode escape is not a scalar value"));
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
                    return refusal;
                };
                decoded.push(decoded_char);
            }
            _ => {
                let refusal = Err(invalid("unsupported escape sequence"));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decode_string: returning an error to the caller");
                return refusal;
            }
        }
    }
    Err(invalid("missing closing quote"))
}

/// Applies one key/value pair written inside `[policy]`.
///
/// A repeated key is refused rather than overwritten, keeping the enforcement
/// flag independent of line order. `repository` uses the shared strict string
/// decoder rather than stripping quotes without interpreting escapes.
pub(super) fn apply_policy_pair(
    key: &str,
    value: &str,
    line_no: usize,
    reader: &mut Reader<'_>,
) -> Result<(), ContractError> {
    let seen_keys = &mut reader.policy_keys;
    let declaration = POLICY_DECLARATION_KEYS
        .iter()
        .copied()
        .find(|candidate| *candidate == key);
    if !POLICY_KEYS.contains(&key) && declaration.is_none() {
        let refusal = Err(ContractError::UnknownKey {
            line: line_no,
            key: key.to_owned(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "apply_policy_pair: returning an error to the caller");
        return refusal;
    }
    if seen_keys.iter().any(|seen| seen == key) {
        let refusal = Err(ContractError::DuplicatePolicyKey {
            line: line_no,
            key: key.to_owned(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "apply_policy_pair: returning an error to the caller");
        return refusal;
    }
    if let Some(declared) = declaration {
        let decoded = decode_string(value, key, line_no)?;
        seen_keys.push(key.to_owned());
        reader.policy_fields.push(RawField {
            key: declared,
            value: decoded,
            line: line_no,
        });
        return Ok(());
    }
    match key {
        "enforce" => {
            let decoded = decode_enforce(value, line_no)?;
            seen_keys.push(key.to_owned());
            reader.enforce = decoded;
            Ok(())
        }
        "schema" => {
            let decoded = decode_schema(value, line_no)?;
            seen_keys.push(key.to_owned());
            reader.schema = decoded;
            Ok(())
        }
        _ => {
            let decoded = decode_string(value, key, line_no)?;
            seen_keys.push(key.to_owned());
            reader.repository = Some(decoded);
            Ok(())
        }
    }
}

/// Applies one schema-approved key/value pair to the current repeated entry.
///
/// Repeated keys and unsupported string spellings are refusals, not precedence
/// rules, so line order cannot change the effective approval.
pub(super) fn apply_entry_pair(
    key: &str,
    value: &str,
    line_no: usize,
    draft: &mut RawEntry,
    allowed_keys: &[&'static str],
) -> Result<(), ContractError> {
    let known = allowed_keys
        .iter()
        .find(|candidate| **candidate == key)
        .ok_or_else(|| ContractError::UnknownKey {
            line: line_no,
            key: key.to_owned(),
        })?;
    if let Some(previous) = draft.fields.iter().find(|field| field.key == *known) {
        let refusal = Err(ContractError::DuplicateEntryKey {
            key: key.to_owned(),
            entry_header: draft.entry_header.clone(),
            entry_index: draft.index,
            entry_line: draft.line,
            first_line: previous.line,
            duplicate_line: line_no,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "apply_entry_pair: returning an error to the caller");
        return refusal;
    }
    let decoded = decode_string(value, key, line_no)?;
    draft.fields.push(RawField {
        key: known,
        value: decoded,
        line: line_no,
    });
    Ok(())
}

/// Dispatches a key/value pair according to the section the reader is inside.
///
/// `drafts` is a slice rather than a `Vec` because this function never grows the
/// list: only `handle_section_header` may push, and it runs first.
pub(super) fn process_pair(
    reader: &mut Reader<'_>,
    key: &str,
    value: &str,
    line_no: usize,
) -> Result<(), ContractError> {
    // `Section` is a fieldless enum, so matching the dereferenced value copies
    // nothing and needs no `ref` bindings.
    match reader.section {
        Section::None => Err(ContractError::OrphanKey {
            line: line_no,
            key: key.to_owned(),
        }),
        Section::Policy => apply_policy_pair(key, value, line_no, reader),
        Section::Retire => {
            // The retire table only ever follows its approval's header, which
            // pushed the draft this table extends; a missing draft is the
            // parser's own defect, refused rather than unwrapped.
            let draft = reader
                .entries
                .last_mut()
                .ok_or_else(|| ContractError::OrphanKey {
                    line: line_no,
                    key: key.to_owned(),
                })?;
            crate::retire_read::apply_pair(key, value, line_no, draft)
        }
        Section::Entry => {
            // `Section::Entry` is only ever entered by `handle_section_header`
            // pushing a draft, and a draft is never popped, so this is a failure
            // the type system cannot express away. Resolving it as an orphan key
            // rather than asserting keeps the reader fail-closed on a parser bug
            // instead of aborting the process mid-register.
            let draft = reader
                .entries
                .last_mut()
                .ok_or_else(|| ContractError::OrphanKey {
                    line: line_no,
                    key: key.to_owned(),
                })?;
            apply_entry_pair(key, value, line_no, draft, reader.allowed_keys)
        }
    }
}

/// Reads one raw line of the register: strips any trailing comment, skips a
/// blank or comment-only line, consumes a section header, and otherwise requires
/// a well-formed `key = value` pair.
///
/// A line that is neither is `Malformed` rather than skipped, which is the
/// property that makes the register fail-closed. `index` is the zero-based
/// position from `enumerate`; the reported number is one-based.
pub(super) fn process_contract_line(
    raw: &str,
    index: usize,
    reader: &mut Reader<'_>,
) -> Result<(), ContractError> {
    // Diagnostics are one-based. `index` comes from `enumerate` over a string's
    // lines, so it is strictly less than the input length and cannot be
    // `usize::MAX`; saturating rather than wrapping guarantees the reported line
    // number stays monotonic even on that impossible input.
    let line_no = index.saturating_add(1);
    let line = strip_comment(raw).trim();
    if line.is_empty() {
        return Ok(());
    }
    if handle_section_header(
        line,
        line_no,
        reader.entry_header,
        &mut reader.section,
        &mut reader.policy_declared,
        &mut reader.entries,
    )? {
        return Ok(());
    }
    let (key, value) = split_pair(line).ok_or_else(|| ContractError::Malformed {
        line: line_no,
        text: line.to_owned(),
    })?;
    process_pair(reader, key, value, line_no)
}

/// The syntax-only result shared by the dependency and invariant registers.
///
/// The line reader owns TOML's small common subset; each register then applies
/// its own schema and repository-aware semantic checks to these raw entries.
pub(crate) struct RawRegister {
    /// Whether the register asks the caller to make refusals fatal.
    pub(crate) enforce: bool,
    /// Optional repository authority carried by the dependency register.
    pub(crate) repository: Option<String>,
    /// Register schema version; 1 when absent.
    pub(crate) schema: u32,
    /// `[policy]` declarations from [`POLICY_DECLARATION_KEYS`], in source order.
    pub(crate) policy_fields: Vec<RawField>,
    /// Repeated blocks in source order.
    pub(crate) entries: Vec<RawEntry>,
}

/// Parses the shared register shape without applying an entry schema.
///
/// `entry_header` selects the repeated TOML table and `allowed_keys` is the
/// register-specific schema. Keeping both parameters here means the invariant
/// register cannot silently grow a second line parser while the dependency
/// register retains its existing fail-closed behaviour.
pub(crate) fn parse_register(
    text: &str,
    entry_header: &str,
    allowed_keys: &[&'static str],
) -> Result<RawRegister, ContractError> {
    let mut reader = Reader {
        entry_header,
        allowed_keys,
        section: Section::None,
        policy_declared: false,
        policy_keys: Vec::new(),
        enforce: true,
        repository: None,
        schema: 1,
        policy_fields: Vec::new(),
        entries: Vec::new(),
    };

    for (index, raw) in text.lines().enumerate() {
        process_contract_line(raw, index, &mut reader)?;
    }

    Ok(RawRegister {
        enforce: reader.enforce,
        repository: reader.repository,
        schema: reader.schema,
        policy_fields: reader.policy_fields,
        entries: reader.entries,
    })
}

/// Refuses a second approval for the same `(crate, owner, capability)` triple.
///
/// The comparison normalises crate and owner names through `normalise` but
/// compares capability byte-for-byte: `-`/`_` drift is a Cargo naming artefact
/// and is tolerated, while a capability is a semantic name this module controls
/// and a near-miss there is a real difference.
pub(super) fn check_duplicate_entry(
    entries: &[Entry],
    entry: &Entry,
    line: usize,
) -> Result<(), ContractError> {
    // Identity is byte-exact. Folding `-`/`_` here would let `foo-bar` and
    // `foo_bar` collide into one entry, which is the conflation the exact match
    // in `admits` exists to prevent.
    let duplicate = entries.iter().any(|existing| {
        existing.krate == entry.krate
            && existing.owner == entry.owner
            && existing.capability == entry.capability
    });
    if duplicate {
        Err(ContractError::DuplicateEntry {
            krate: entry.krate.clone(),
            line,
        })
    } else {
        Ok(())
    }
}

/// Refuses an alias that two distinct Cargo packages would share.
///
/// Every entry's exact `krate` is registered first as its own owner, so an
/// alias that equals another package's real name is caught as a collision, and
/// two entries claiming the same alias are caught too. The alias must also be a
/// valid identifier and must not be the entry's own crate name.
pub(super) fn check_aliases(entries: &[Entry]) -> Result<(), ContractError> {
    let mut owners: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for entry in entries {
        owners
            .entry(entry.krate.as_str())
            .or_insert(entry.krate.as_str());
    }
    for entry in entries {
        for alias in &entry.aliases {
            match owners.get(alias.as_str()) {
                Some(owner) if *owner != entry.krate => {
                    let refusal = Err(ContractError::AliasCollision {
                        alias: alias.clone(),
                        owner: (*owner).to_owned(),
                        line: entry.line,
                    });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_aliases: returning an error to the caller");
                    return refusal;
                }
                Some(_) => {}
                None => {
                    owners.insert(alias.as_str(), entry.krate.as_str());
                }
            }
        }
    }
    Ok(())
}
