//! From raw blocks to entries and policy: every field rule, the fingerprint,
//! and the comment and pair splitting the reader leans on.

use super::read::{REQUIRED, RawEntry, RawField};
use super::{ContractError, Entry, Frozen, Policy, Tier};

/// A 128-bit FNV-1a fingerprint of `text`, as `fnv1a128:<32 hex>`.
///
/// Chosen over a std `Hasher` because `DefaultHasher`'s algorithm is explicitly
/// unspecified and may change between releases, which would make a stored
/// receipt digest meaningless across toolchains. FNV-1a is a fixed,
/// well-defined function of the bytes and costs one multiply per byte over a
/// register of a few kilobytes.
pub(super) fn fingerprint(text: &str) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for byte in text.as_bytes() {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("fnv1a128:{hash:032x}")
}

/// The line a diagnostic about `field` names.
///
/// A field the block wrote has its own line. A field it never wrote has none,
/// and the diagnostic is then about the *block* — the repair is to add the key
/// there — so the block's opening line is the position that tells the reader
/// what to do. Both cases are stated once here rather than resolved by a
/// substituted line at each of the three sites that need one.
pub(super) fn field_position(draft: &RawEntry, field: &str) -> usize {
    match draft.field_line(field) {
        Some(line) => line,
        None => draft.line(),
    }
}

/// Requires a field to be present and non-blank.
///
/// Whitespace only counts as blank, so `owner = "   "` is refused exactly as a
/// missing `owner` would be: a placeholder is not evidence.
pub(crate) fn check_field_present(
    draft: &RawEntry,
    krate: Option<&str>,
    field: &'static str,
) -> Result<(), ContractError> {
    if draft
        .get(field)
        .is_some_and(|value| !value.trim().is_empty())
    {
        Ok(())
    } else {
        Err(ContractError::MissingField {
            krate: krate.map(str::to_owned),
            field,
            line: draft.line(),
        })
    }
}

/// Rejects the first required field that is absent or blank, in `REQUIRED`
/// order. `krate` is only used to name the offending block in the message.
pub(super) fn validate_required_fields(
    draft: &RawEntry,
    krate: Option<&str>,
) -> Result<(), ContractError> {
    for field in REQUIRED {
        check_field_present(draft, krate, field)?;
    }
    Ok(())
}

/// Decodes `tier`, refusing anything outside `boundary` / `vendor`.
///
/// Callers run this after `validate_required_fields`, so an absent `tier` is
/// reported as `MissingField` before it can reach `Tier::parse`; the absent
/// branch here is typed rather than asserted so a reordering bug surfaces as a
/// refusal instead of a panic.
pub(super) fn validate_tier(draft: &RawEntry, krate: &str) -> Result<Tier, ContractError> {
    let tier_text = draft.require("tier", Some(krate))?;
    Tier::parse(tier_text).ok_or_else(|| ContractError::BadTier {
        line: draft.line,
        value: tier_text.to_owned(),
    })
}

/// Requires `approved_on` to be a real Gregorian `YYYY-MM-DD` date.
///
/// The field makes an approval attributable to a review, not a scheduled event.
pub(super) fn validate_date(
    approved_on: &str,
    krate: &str,
    line: usize,
) -> Result<(), ContractError> {
    if is_iso_date(approved_on) {
        Ok(())
    } else {
        Err(ContractError::BadDate {
            krate: krate.to_owned(),
            line,
            value: approved_on.to_owned(),
        })
    }
}

/// Requires `reason` to clear the `is_a_sentence` bar, refusing the two shapes
/// that make an approval a whitelist entry: a one-word reason and a reason that
/// merely restates the crate's name.
pub(super) fn validate_reason(reason: &str, krate: &str) -> Result<(), ContractError> {
    if is_a_sentence(reason, krate) {
        Ok(())
    } else {
        Err(ContractError::ThinReason {
            krate: krate.to_owned(),
        })
    }
}

/// Checks Cargo's portable package/owner identifier subset.
pub(crate) fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

/// Tests whether `value` is a legible SPDX licence expression.
///
/// Deliberately structural rather than semantic. The gate does not ship an
/// SPDX parser, and inventing one would make the accepted-licence set a
/// judgement call in code; what it *can* check without a table is the shape a
/// licence field must have before it is worth comparing byte-for-byte against
/// what Cargo reports (#208):
///
/// - at least one identifier;
/// - `AND` / `OR` / `WITH` in upper case only, and never leading or trailing;
/// - no empty operand, so `OR MIT`, `MIT OR` and `MIT  OR` are all refused;
/// - parenthesised groups kept together, so `(MIT OR Apache-2.0) AND
///   Unicode-3.0` parses the way a reader parses it;
/// - identifiers made of letters, digits, `.`, `-` and `_`, which covers every
///   SPDX identifier and licence-reference suffix this repository can meet.
///
/// Whether the identifiers are *accepted* is a separate question, answered by
/// [`Contract::accepted_licenses`] in the audit rather than here: an expression can
/// be perfectly legible and still name a licence this repository does not take.
pub(super) fn is_spdx_expression(value: &str) -> bool {
    // SPDX separates tokens with exactly one space; a doubled, padded or
    // tab-separated expression is a different string from the one the manifest
    // declares, so it cannot be recorded as if the two compared equal.
    if value != value.trim()
        || value.contains("  ")
        || value
            .chars()
            .any(|ch| ch.is_ascii_whitespace() && ch != ' ')
    {
        return false;
    }
    let mut operands = 0_usize;
    let mut depth = 0_usize;
    let mut previous_operand = false;
    let spaced = value.replace('(', " ( ").replace(')', " ) ");
    for token in spaced.split_ascii_whitespace() {
        if token == "(" {
            if previous_operand {
                return false;
            }
            depth = depth.saturating_add(1);
        } else if token == ")" {
            if !previous_operand || depth == 0 {
                return false;
            }
            depth = depth.saturating_sub(1);
            // A closed group is an operand, so `(MIT)(Apache-2.0)` is refused at
            // the second `(` and `(MIT OR Apache-2.0)` is whole.
            previous_operand = true;
        } else if matches!(token, "AND" | "OR" | "WITH") {
            if !previous_operand {
                return false;
            }
            previous_operand = false;
        } else {
            if previous_operand {
                return false;
            }
            let identifier = token;
            if identifier.is_empty()
                || !identifier
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
            {
                return false;
            }
            previous_operand = true;
            operands = operands.saturating_add(1);
        }
    }
    operands > 0 && depth == 0 && previous_operand
}

/// Whether an authored `origin` names a supported origin identity.
///
/// Three shapes are admitted: a Cargo registry source (`registry+`), a Cargo
/// sparse-registry source (`sparse+`), and a Cargo Git source (`git+`). A
/// scheme-free, non-empty string is an external path authority. Everything
/// else — in particular an unknown scheme such as `svn+…` — is refused at load
/// rather than stored as an origin that would later compare as an ordinary
/// admitted string.
pub(super) fn is_supported_origin(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if value.starts_with("registry+") || value.starts_with("sparse+") || value.starts_with("git+") {
        return true;
    }
    !value.contains("://") && !value.contains('+')
}

/// Refuses a decoded field that is outside its schema vocabulary.
pub(super) fn validate_closed_value(
    draft: &RawEntry,
    krate: &str,
    field: &'static str,
    value: &str,
    valid: bool,
    expected: &'static str,
) -> Result<(), ContractError> {
    if valid {
        Ok(())
    } else {
        Err(ContractError::InvalidField {
            krate: krate.to_owned(),
            field,
            line: field_position(draft, field),
            value: value.to_owned(),
            expected,
        })
    }
}

/// Reads an optional Boolean field under the closed `true`/`false` grammar.
///
/// A value outside the grammar is a typed field refusal, never a default: an
/// unknown spelling must not silently stand a policy down.
pub(super) fn optional_bool(
    draft: &RawEntry,
    key: &'static str,
    krate: &str,
) -> Result<Option<bool>, ContractError> {
    match draft.get(key) {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(value) => Err(ContractError::InvalidField {
            krate: krate.to_owned(),
            field: key,
            line: field_position(draft, key),
            value: value.to_owned(),
            expected: "expected true or false",
        }),
    }
}

/// Reads an optional comma-separated list of Cargo feature names.
///
/// Presence is meaningful: an absent key leaves the dimension unconstrained
/// (grandfathered), while a present key admits exactly the features it lists.
pub(super) fn optional_feature_list(
    draft: &RawEntry,
    key: &'static str,
    krate: &str,
) -> Result<Option<Vec<String>>, ContractError> {
    let Some(raw) = draft.get(key) else {
        return Ok(None);
    };
    let items = split_csv(raw);
    validate_closed_value(
        draft,
        krate,
        key,
        &items.join(","),
        !items.is_empty() && items.iter().all(|item| is_feature_name(item)),
        "expected a nonempty comma-separated list of Cargo feature names",
    )?;
    Ok(Some(items))
}

/// Checks a Cargo feature spelling: a non-empty run of name characters.
///
/// Feature names may be plain (`std`), versioned-dependency features
/// (`serde/derive`), or explicit dependency activations (`dep:syn`), so the
/// admit set is alphanumerics plus the separators Cargo uses.
pub(super) fn is_feature_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '+'))
}

/// Turns a fully-read draft into an `Entry`, validating every field rule.
///
/// The checks run in a fixed order (required fields, then tier, then date,
/// then reason), so a register with several defects always reports the same
/// one. `crate` falls back to `<unnamed>` for diagnostics only; a block whose
/// `crate` is absent still fails `validate_required_fields` immediately after.
pub(super) fn build(draft: &RawEntry) -> Result<Entry, ContractError> {
    // A block that names no crate is refused here, at its own line, naming the
    // field it lacks — before any other field is read, so nothing else in the
    // block is interpreted under a name the register did not write.
    let Some(krate) = draft.get("crate").map(str::to_owned) else {
        let refusal = Err(ContractError::MissingField {
            krate: None,
            field: "crate",
            line: draft.line(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), line = draft.line(), "build: an approval block names no crate");
        return refusal;
    };
    validate_required_fields(draft, Some(&krate))?;
    let tier = validate_tier(draft, &krate)?;
    let approved_on = draft.require("approved_on", Some(&krate))?.to_owned();
    validate_date(&approved_on, &krate, field_position(draft, "approved_on"))?;
    let reason = draft.require("reason", Some(&krate))?.to_owned();
    validate_reason(&reason, &krate)?;

    // Every remaining field is read while `krate` is still borrowable; the
    // struct literal below moves `krate`, so nothing may borrow it afterwards.
    let version = draft.require("version", Some(&krate))?.to_owned();
    let owner = draft.require("owner", Some(&krate))?.to_owned();
    let capability = draft.require("capability", Some(&krate))?.to_owned();
    let license = draft.require("license", Some(&krate))?.to_owned();
    let source = draft.require("source", Some(&krate))?.to_owned();
    let origin = draft.get("origin").map(str::to_owned);
    let features = optional_feature_list(draft, "features", &krate)?;
    let required_features = optional_feature_list(draft, "required_features", &krate)?;
    let uses_default_features = optional_bool(draft, "uses_default_features", &krate)?;
    let optional = optional_bool(draft, "optional", &krate)?;
    let target = draft.get("target").map(str::to_owned);
    // An entry that authors no `aliases` line admits exactly the spelling it
    // names as `crate`, which is what an empty list admits too: INV-DEP-13
    // grandfathers a dimension an entry does not author, and for this dimension
    // "not authored" and "no aliases beyond `crate`" are one admission. The two
    // are not the same *repair*, and `field_position` is what tells them apart
    // in a diagnostic. Refusing the absent key instead would refuse every
    // register that predates it — this repository's own 33 entries author no
    // alias list — so absence is read here rather than refused.
    let aliases = match draft.get("aliases") {
        Some(list) => split_csv(list),
        None => Vec::new(),
    };
    let allowed_consumers = split_csv(draft.require("allowed_consumers", Some(&krate))?);
    let allowed_kinds = split_csv(draft.require("allowed_kinds", Some(&krate))?);
    validate_closed_value(
        draft,
        &krate,
        "crate",
        &krate,
        is_identifier(&krate),
        "expected a Cargo package identifier using ASCII letters, digits, hyphens or underscores",
    )?;
    validate_closed_value(
        draft,
        &krate,
        "owner",
        &owner,
        is_identifier(&owner),
        "expected a workspace package identifier using ASCII letters, digits, hyphens or underscores",
    )?;
    validate_closed_value(
        draft,
        &krate,
        "license",
        &license,
        is_spdx_expression(&license),
        "expected an SPDX licence expression, for example `MIT OR Apache-2.0`",
    )?;
    validate_closed_value(
        draft,
        &krate,
        "source",
        &source,
        matches!(source.as_str(), "registry" | "git" | "path"),
        "expected one of registry, git or path",
    )?;
    if let Some(origin) = origin.as_deref() {
        validate_closed_value(
            draft,
            &krate,
            "origin",
            origin,
            is_supported_origin(origin),
            "expected a registry+ / sparse+ / git+ Cargo source, or a scheme-free path authority",
        )?;
    }
    validate_closed_value(
        draft,
        &krate,
        "allowed_consumers",
        &allowed_consumers.join(","),
        !allowed_consumers.is_empty() && allowed_consumers.iter().all(|item| is_identifier(item)),
        "expected a nonempty comma-separated list of package identifiers",
    )?;
    validate_closed_value(
        draft,
        &krate,
        "allowed_kinds",
        &allowed_kinds.join(","),
        !allowed_kinds.is_empty()
            && allowed_kinds
                .iter()
                .all(|kind| matches!(kind.as_str(), "normal" | "build" | "dev")),
        "expected a nonempty list containing only normal, build or dev",
    )?;
    if let Some(target) = target.as_deref() {
        // An empty target is meaningful: it requires an unconditional
        // declaration. Any non-empty value is the `cfg(…)` or triple Cargo
        // reported, compared byte-for-byte at admission.
        validate_closed_value(
            draft,
            &krate,
            "target",
            target,
            !target.chars().any(char::is_control),
            "expected a target cfg expression, possibly empty for unconditional",
        )?;
    }
    validate_closed_value(
        draft,
        &krate,
        "aliases",
        &aliases.join(","),
        aliases
            .iter()
            .all(|alias| is_identifier(alias) && alias != &krate),
        "expected comma-separated package identifiers, none equal to crate",
    )?;
    if let (Some(allowed), Some(required)) = (features.as_ref(), required_features.as_ref()) {
        validate_closed_value(
            draft,
            &krate,
            "required_features",
            &required.join(","),
            required.iter().all(|needed| allowed.contains(needed)),
            "expected a subset of the allowed features",
        )?;
    }
    let approved_by = draft.require("approved_by", Some(&krate))?.to_owned();
    let review = draft.require("review", Some(&krate))?.to_owned();
    let retire = crate::retire_read::build(draft, &krate)?;

    Ok(Entry {
        krate,
        tier,
        version,
        owner,
        capability,
        license,
        source,
        origin,
        features,
        required_features,
        uses_default_features,
        optional,
        target,
        aliases,
        allowed_consumers,
        allowed_kinds,
        reason,
        approved_by,
        approved_on,
        review,
        retire,
        line: draft.line,
    })
}

/// Decodes the repository-policy declarations of a dependency register.
///
/// Every list is a comma-separated basic string, refused whole when a member is
/// empty, repeated, or not in its vocabulary — a list that silently dropped a
/// malformed member would admit less, or more, than the reviewer read.
pub(super) fn build_policy(fields: &[RawField]) -> Result<Policy, ContractError> {
    let mut policy = Policy::default();
    let mut frozen_surfaces: Option<(Vec<String>, usize)> = None;
    let mut frozen_tier: Option<Tier> = None;
    for field in fields {
        let decoded = decode_policy_field(field);
        let value = match decoded {
            Ok(value) => value,
            Err(error) => {
                let refusal = Err(error);
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "build_policy: returning an error to the caller");
                return refusal;
            }
        };
        match value {
            PolicyValue::Licences(list) => policy.accepted_licenses = Some(list),
            PolicyValue::Surfaces(list) => policy.surfaces = Some(list),
            PolicyValue::Frozen(list) => frozen_surfaces = Some((list, field.line)),
            PolicyValue::Tier(tier) => frozen_tier = Some(tier),
        }
    }
    let missing = match (frozen_surfaces.is_some(), frozen_tier.is_some()) {
        (true, false) => Some(("frozen_surfaces", "frozen_tier")),
        (false, true) => Some(("frozen_tier", "frozen_surfaces")),
        _ => None,
    };
    if let Some((present, missing)) = missing {
        let refusal = Err(ContractError::IncompletePolicy { present, missing });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "build_policy: returning an error to the caller");
        return refusal;
    }
    if let (Some((surfaces, line)), Some(tier)) = (frozen_surfaces, frozen_tier) {
        let checked = check_frozen_are_surfaces(&surfaces, policy.surfaces.as_deref(), line);
        checked?;
        policy.frozen = Some(Frozen { surfaces, tier });
    }
    Ok(policy)
}

/// One decoded `[policy]` declaration, before the freeze's two halves are
/// paired.
pub(super) enum PolicyValue {
    /// `accepted_licenses`.
    Licences(Vec<String>),
    /// `surfaces`.
    Surfaces(Vec<String>),
    /// `frozen_surfaces`.
    Frozen(Vec<String>),
    /// `frozen_tier`.
    Tier(Tier),
}

/// Decodes one declaration against its own vocabulary.
pub(super) fn decode_policy_field(field: &RawField) -> Result<PolicyValue, ContractError> {
    match field.key {
        "accepted_licenses" => policy_list(field, is_spdx_term).map(PolicyValue::Licences),
        "surfaces" => policy_list(field, is_identifier).map(PolicyValue::Surfaces),
        "frozen_surfaces" => policy_list(field, is_identifier).map(PolicyValue::Frozen),
        _ => Tier::parse(&field.value)
            .map(PolicyValue::Tier)
            .ok_or_else(|| ContractError::BadTier {
                line: field.line,
                value: field.value.clone(),
            }),
    }
}

/// A frozen surface the register's own closed surface set does not name is a
/// contradiction in one block, refused at load rather than audited around.
pub(super) fn check_frozen_are_surfaces(
    frozen: &[String],
    surfaces: Option<&[String]>,
    line: usize,
) -> Result<(), ContractError> {
    let Some(surfaces) = surfaces else {
        return Ok(());
    };
    match frozen.iter().find(|name| !surfaces.contains(name)) {
        Some(outside) => Err(ContractError::BadPolicyValue {
            line,
            key: "frozen_surfaces".to_owned(),
            value: format!("{outside} (not one of the declared surfaces)"),
        }),
        None => Ok(()),
    }
}

/// Splits one policy list and refuses it whole on an empty, repeated or
/// out-of-vocabulary member.
pub(super) fn policy_list(
    field: &RawField,
    member: fn(&str) -> bool,
) -> Result<Vec<String>, ContractError> {
    let refuse = || ContractError::BadPolicyValue {
        line: field.line,
        key: field.key.to_owned(),
        value: field.value.clone(),
    };
    let items = split_csv(&field.value);
    let malformed = items.is_empty()
        || items.iter().enumerate().any(|(index, item)| {
            items.iter().take(index).any(|earlier| earlier == item) || !member(item)
        });
    if malformed {
        let refusal = Err(refuse());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "policy_list: returning an error to the caller");
        return refusal;
    }
    Ok(items)
}

/// Tests whether `term` is one SPDX licence term: an identifier, or an
/// identifier `WITH` an exception identifier.
///
/// An identifier is ASCII letters, digits, `.`, `-` and `+`, which spans every
/// SPDX list identifier and `LicenseRef-*`. This is the shape, not the SPDX
/// list: which identifiers a repository accepts is exactly the decision this
/// list records, so the reader admits any well-formed one.
pub(super) fn is_spdx_term(term: &str) -> bool {
    let identifier = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
            && !matches!(word, "AND" | "OR" | "WITH")
    };
    let mut words = term.split(' ');
    match (words.next(), words.next(), words.next(), words.next()) {
        (Some(single), None, None, None) => identifier(single),
        (Some(licence), Some("WITH"), Some(exception), None) => {
            identifier(licence) && identifier(exception)
        }
        _ => false,
    }
}

/// Splits a comma-separated list, retaining empty members for schema refusal.
///
/// One final comma remains a supported legacy terminator; interior empty
/// members are never discarded because they would hide malformed lists.
pub(super) fn split_csv(value: &str) -> Vec<String> {
    let mut items = value.split(',').map(str::trim).collect::<Vec<_>>();
    if value.trim_end().ends_with(',') && items.last().is_some_and(|item| item.is_empty()) {
        items.pop();
    }
    items.into_iter().map(str::to_owned).collect()
}

// ── Field rules ─────────────────────────────────────────────────────────────

/// Tests whether `reason` is a sentence that says something.
///
/// Three rules, all of which must hold: at least 24 characters after trimming,
/// a trailing full stop, at least four whitespace-separated words, and, after
/// stripping the full stop and normalising, the text must not simply repeat the
/// crate's name. The floor is deliberately low enough to pass any honest
/// justification and high enough to fail `reason = "needed"` or
/// `reason = "serde"`.
pub(super) fn is_a_sentence(reason: &str, krate: &str) -> bool {
    let trimmed = reason.trim();
    if trimmed.len() < 24 || !trimmed.ends_with('.') {
        return false;
    }
    if trimmed.split_whitespace().count() < 4 {
        return false;
    }
    fold_for_prose(trimmed.trim_end_matches('.')) != fold_for_prose(krate)
}

/// Validates a canonical Gregorian `YYYY-MM-DD` date without allocation.
///
/// Length is checked before indexing; decimal fields are bounded by the fixed
/// format, and month length follows the Gregorian leap-year rule.
pub(crate) fn is_iso_date(value: &str) -> bool {
    let date_bytes = value.as_bytes();
    if date_bytes.len() != 10
        || date_bytes[4] != b'-'
        || date_bytes[7] != b'-'
        || ![0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&position| date_bytes[position].is_ascii_digit())
    {
        return false;
    }
    let year = decimal_pair(&date_bytes[0..4]);
    let month = decimal_pair(&date_bytes[5..7]);
    let day = decimal_pair(&date_bytes[8..10]);
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= month_days
}

/// Parses an ASCII decimal byte slice without allocation.
pub(super) fn decimal_pair(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u32::from(byte.checked_sub(b'0')?)))
    })
}

/// Folds a string for the *prose* check in [`is_a_sentence`] and nothing else.
///
/// Trims, lowercases, and maps `-` to `_`. This is deliberately **not** used
/// for package identity: Cargo's `-`/`_` fold decides whether two published
/// names collide, but it is not the identity a register approves. Approval
/// matching is byte-exact (see [`Entry::admits`]); the only place the fold
/// survives is deciding whether a `reason` merely restates the crate name,
/// where a near-miss in prose is not an authority question.
pub(super) fn fold_for_prose(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

// ── Line reading ────────────────────────────────────────────────────────────

/// Drops a trailing `#` comment, respecting quotes so a `#` inside a reason or
/// a review URL survives.
///
/// Quote tracking is deliberately shallow: a backslash escapes the next byte
/// only while inside quotes, so an odd number of quotes leaves the rest of the
/// line quoted and the `#` is kept rather than truncated. Erring toward keeping
/// the `#` is the safe direction: the value then fails validation visibly
/// instead of being silently cut short.
pub(super) fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_quotes => escaped = true,
            b'"' => in_quotes = !in_quotes,
            b'#' if !in_quotes => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Splits a `key = value` line, returning `None` for anything else.
///
/// The key must be non-empty and made only of ASCII alphanumerics and `_`, which
/// is what makes a malformed line a refusal rather than a silently ignored key.
/// Splitting is on the first `=`, so a value may itself contain `=`: a review
/// URL or an extra constraint needs no escaping.
pub(super) fn split_pair(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some((key, value.trim()))
}
