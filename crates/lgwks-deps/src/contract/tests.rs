use super::*;
use std::fmt::Write as FmtWrite;

/// What every test here returns.
///
/// The workspace forbids `unwrap`/`expect` outright, with no test exemption,
/// so a test propagates its failure with `?` rather than aborting the run.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The `[policy]` block: enforcement, its Boolean grammar and its placement.
mod policy;

fn entry(extra: &str) -> String {
    format!(
        concat!(
            "[[approved]]\n",
            "crate = \"serde\"\n",
            "tier = \"boundary\"\n",
            "version = \"1.0\"\n",
            "owner = \"lgwks_std\"\n",
            "capability = \"json.serialization\"\n",
            "license = \"MIT OR Apache-2.0\"\n",
            "source = \"registry\"\n",
            "allowed_consumers = \"lgwks_std\"\n",
            "allowed_kinds = \"normal\"\n",
            "reason = \"Derive-based serialization needs compiler introspection std does not expose.\"\n",
            "approved_by = \"reviewer\"\n",
            "approved_on = \"2026-08-19\"\n",
            "review = \"docs/ADMISSION.md\"\n",
            "{}"
        ),
        extra
    )
}

fn complete(input: &str) -> String {
    let mut output = String::new();
    for line in input.lines() {
        output.push_str(line);
        output.push('\n');
        if line.trim_start().starts_with("version =") {
            output.push_str(
                "owner = \"lgwks_std\"\n\
                 capability = \"json.serialization\"\n\
                 license = \"MIT OR Apache-2.0\"\n\
                 source = \"registry\"\n\
                 allowed_consumers = \"lgwks_std\"\n\
                 allowed_kinds = \"normal\"\n",
            );
        }
    }
    output
}

#[test]
fn a_complete_entry_parses() -> TestResult {
    let contract = Contract::parse(&entry(""))?;
    assert_eq!(contract.entries.len(), 1);
    let parsed = &contract.entries[0];
    assert_eq!(parsed.krate, "serde");
    assert_eq!(parsed.tier, Tier::Boundary);
    assert_eq!(parsed.version, "1.0");
    assert_eq!(parsed.owner, "lgwks_std");
    assert_eq!(parsed.capability, "json.serialization");
    assert_eq!(parsed.license, "MIT OR Apache-2.0");
    assert_eq!(parsed.allowed_consumers, ["lgwks_std"]);
    assert!(parsed.reason.ends_with('.'));
    assert_eq!(parsed.approved_by, "reviewer");
    assert_eq!(parsed.approved_on, "2026-08-19");
    assert_eq!(parsed.review, "docs/ADMISSION.md");
    Ok(())
}

#[test]
fn duplicate_entry_keys_refuse_both_identical_and_conflicting_values() -> TestResult {
    let assignments = [
        ("crate", "\"serde\"", "\"another-crate\""),
        ("tier", "\"boundary\"", "\"vendor\""),
        ("version", "\"1.0\"", "\"2.0\""),
        ("owner", "\"lgwks_std\"", "\"lgwks_deps\""),
        (
            "capability",
            "\"json.serialization\"",
            "\"other.capability\"",
        ),
        ("license", "\"MIT OR Apache-2.0\"", "\"MIT\""),
        ("source", "\"registry\"", "\"git\""),
        ("allowed_consumers", "\"lgwks_std\"", "\"lgwks_deps\""),
        ("allowed_kinds", "\"normal\"", "\"dev\""),
        (
            "reason",
            "\"Derive-based serialization needs compiler introspection std does not expose.\"",
            "\"A different capability has a distinct boundary.\"",
        ),
        ("approved_by", "\"reviewer\"", "\"another-reviewer\""),
        ("approved_on", "\"2026-08-19\"", "\"2026-08-20\""),
        ("review", "\"docs/ADMISSION.md\"", "\"docs/OTHER.md\""),
    ];
    for (key, identical, conflicting) in assignments {
        let original = entry("");
        let reversed = original.replace(
            &format!("{key} = {identical}"),
            &format!("{key} = {conflicting}"),
        );
        for (base, value) in [
            (original.as_str(), identical),
            (original.as_str(), conflicting),
            (reversed.as_str(), identical),
        ] {
            let first_line = base
                .lines()
                .position(|line| line.starts_with(&format!("{key} =")))
                .map(|index| index + 1)
                .ok_or_else(|| format!("fixture has no {key} assignment"))?;
            let duplicate_line = base.lines().count() + 1;
            let input = format!("{base}{key} = {value}\n");
            assert_eq!(
                Contract::parse(&input),
                Err(ContractError::DuplicateEntryKey {
                    key: key.to_owned(),
                    entry_header: "[[approved]]".to_owned(),
                    entry_index: 1,
                    entry_line: 1,
                    first_line,
                    duplicate_line,
                }),
                "duplicate {key} with {value} must refuse both positions"
            );
        }
    }
    Ok(())
}

#[test]
fn unique_fields_are_order_independent() -> TestResult {
    let original = entry("");
    let mut lines = original.lines();
    let header = lines.next().ok_or("entry fixture has no table header")?;
    let mut lines = lines.collect::<Vec<_>>();
    lines.reverse();
    let reordered = format!("{header}\n{}\n", lines.join("\n"));
    let from_original = Contract::parse(&original)?;
    let from_reordered = Contract::parse(&reordered)?;
    // The text fingerprint is a property of the bytes, so it differs by
    // construction; the admitted meaning — policy and every entry — must
    // not.
    assert_eq!(
        (
            from_original.enforce,
            from_original.schema,
            &from_original.entries
        ),
        (
            from_reordered.enforce,
            from_reordered.schema,
            &from_reordered.entries
        ),
        "reordering unique fields must not change the admitted contract"
    );
    Ok(())
}

#[test]
fn basic_string_subset_decodes_escapes_comments_unicode_and_crlf() -> TestResult {
    let input = entry("").replace(
        "review = \"docs/ADMISSION.md\"",
        r#"review = "docs/\u0041#é" # trailing comment"#,
    );
    let parsed = Contract::parse(&input)?;
    assert_eq!(
        parsed.entries[0].review, "docs/A#é",
        "basic Unicode escapes, non-ASCII text and quoted hashes must decode"
    );
    let escaped = entry("").replace(
        "review = \"docs/ADMISSION.md\"",
        r#"review = "docs/\"quoted\"\\literal""#,
    );
    assert_eq!(
        Contract::parse(&escaped)?.entries[0].review,
        "docs/\"quoted\"\\literal",
        "quote and backslash escapes must decode without reinterpretation"
    );
    let crlf = entry("").replace('\n', "\r\n");
    assert_eq!(
        Contract::parse(&crlf)?.entries.len(),
        1,
        "CRLF input must preserve the same one-entry register"
    );
    Ok(())
}

#[test]
fn unsupported_string_forms_refuse_at_the_authored_field() {
    let cases = [
        ("approved_by = \"reviewer\"", "approved_by = reviewer"),
        ("approved_by = \"reviewer\"", "approved_by = \"reviewer"),
        (
            "approved_by = \"reviewer\"",
            "approved_by = \"reviewer\\q\"",
        ),
        (
            "approved_by = \"reviewer\"",
            "approved_by = \"reviewer\" trailing",
        ),
        (
            "approved_by = \"reviewer\"",
            "approved_by = \"\"\"reviewer\"\"\"",
        ),
        (
            "approved_by = \"reviewer\"",
            "approved_by = \"reviewer\\n\"",
        ),
    ];
    for (from, to) in cases {
        let input = entry("").replace(from, to);
        assert!(
            matches!(
                Contract::parse(&input),
                Err(ContractError::InvalidString {
                    line: 12,
                    ref key,
                    ..
                }) if key == "approved_by"
            ),
            "unsupported string form {to:?} must give a typed field refusal"
        );
    }
    let raw_control = format!("approved_by = \"reviewer{}\"", '\u{0001}');
    let input = entry("").replace("approved_by = \"reviewer\"", &raw_control);
    assert!(
        matches!(
            Contract::parse(&input),
            Err(ContractError::InvalidString {
                line: 12,
                ref key,
                ..
            }) if key == "approved_by"
        ),
        "a raw control character must be refused at its authored field"
    );
}

#[test]
fn decoded_schema_values_and_calendar_dates_are_closed() -> TestResult {
    let invalid = [
        (
            "allowed_consumers = \"lgwks_std\"",
            "allowed_consumers = \",,,\"",
            "allowed_consumers",
            9,
        ),
        (
            "allowed_kinds = \"normal\"",
            "allowed_kinds = \",,,\"",
            "allowed_kinds",
            10,
        ),
        (
            "allowed_kinds = \"normal\"",
            "allowed_kinds = \"normal,unknown\"",
            "allowed_kinds",
            10,
        ),
        ("source = \"registry\"", "source = \"unknown\"", "source", 8),
        ("crate = \"serde\"", "crate = \"bad package\"", "crate", 2),
        ("owner = \"lgwks_std\"", "owner = \"bad owner\"", "owner", 5),
    ];
    for (from, to, field, line) in invalid {
        let input = entry("").replace(from, to);
        assert!(
            matches!(
                Contract::parse(&input),
                Err(ContractError::InvalidField { field: found, line: found_line, .. })
                    if found == field && found_line == line
            ),
            "invalid decoded {field} must be refused at line {line}"
        );
    }
    for date in ["2026-13-45", "1900-02-29", "2024-04-31", "0000-01-01"] {
        let input = entry("").replace("2026-08-19", date);
        assert!(
            matches!(
                Contract::parse(&input),
                Err(ContractError::BadDate { line: 13, .. })
            ),
            "impossible date {date} must be refused"
        );
    }
    for date in ["2024-02-29", "2000-02-29", "2026-08-19"] {
        let input = entry("").replace("2026-08-19", date);
        assert_eq!(
            Contract::parse(&input)?.entries[0].approved_on,
            date,
            "valid date {date} must remain supported"
        );
    }
    Ok(())
}

/// One final comma on a list field is a delimiter, not an empty element.
///
/// Both list fields of an entry carry the same rule, so the assertion is
/// written once and told what the single surviving element should be.
fn assert_single_trailing_comma_parsed(actual: &[String], expected: &str) {
    assert_eq!(
        actual,
        [expected],
        "one final comma remains a delimiter rather than an empty element"
    );
}

#[test]
fn a_single_legacy_trailing_list_comma_remains_supported() -> TestResult {
    let input = entry("")
        .replace(
            "allowed_consumers = \"lgwks_std\"",
            "allowed_consumers = \"lgwks_std,\"",
        )
        .replace("allowed_kinds = \"normal\"", "allowed_kinds = \"normal,\"");
    let parsed = Contract::parse(&input)?;
    assert_single_trailing_comma_parsed(&parsed.entries[0].allowed_consumers, "lgwks_std");
    assert_single_trailing_comma_parsed(&parsed.entries[0].allowed_kinds, "normal");
    Ok(())
}

#[test]
fn invariant_register_uses_the_shared_duplicate_key_refusal() -> TestResult {
    let fields = [
        ("id", "\"INV-DEP-TEST\"", "\"INV-DEP-OTHER\""),
        (
            "statement",
            "\"The implementation preserves this reviewed contract.\"",
            "\"A different invariant has its own exact statement.\"",
        ),
        ("scope", "\"lgwks_deps\"", "\"lgwks_std\""),
        ("owner", "\"lgwks_deps\"", "\"lgwks_bot\""),
        ("enforcement", "\"static-check\"", "\"monitor\""),
        ("enforced_by", "\"src/lib.rs\"", "\"src/contract.rs\""),
        ("approved_by", "\"reviewer\"", "\"second-reviewer\""),
        ("approved_on", "\"2026-09-20\"", "\"2026-09-21\""),
        ("review", "\"src/lib.rs\"", "\"src/contract.rs\""),
        ("evidence_revision", "\"abcd1234\"", "\"1234abcd\""),
        (
            "evidence_invocation",
            "\"cargo check --locked\"",
            "\"cargo nextest run --locked\"",
        ),
        ("evidence_result", "\"pass\"", "\"fail\""),
    ];
    for (key, original, conflicting) in fields {
        let mut base = String::from("[[invariant]]\n");
        for (field, value, _) in fields {
            writeln!(base, "{field} = {value}")?;
        }
        let reversed = base.replace(
            &format!("{key} = {original}"),
            &format!("{key} = {conflicting}"),
        );
        for (document, duplicate) in [
            (base.as_str(), original),
            (base.as_str(), conflicting),
            (reversed.as_str(), original),
        ] {
            let first_line = document
                .lines()
                .position(|line| line.starts_with(&format!("{key} =")))
                .map(|index| index + 1)
                .ok_or_else(|| format!("fixture has no invariant {key}"))?;
            let duplicate_line = document.lines().count() + 1;
            let input = format!("{document}{key} = {duplicate}\n");
            let error = crate::invariants::Register::parse(&input)
                .err()
                .ok_or_else(|| format!("invariant parser accepted duplicate {key}"))?;
            let diagnostic = error.to_string();
            assert!(
                diagnostic.contains(&format!("first assignment is at line {first_line}")),
                "invariant duplicate {key} must retain the first source position: {diagnostic}"
            );
            assert!(
                diagnostic.contains(&format!("line {duplicate_line}"))
                    && diagnostic.contains("duplicate key")
                    && diagnostic.contains(key),
                "invariant duplicate {key} must identify the key and second position: {diagnostic}"
            );
        }
    }
    Ok(())
}

#[test]
fn comments_and_blank_lines_are_ignored() -> TestResult {
    let input = concat!(
        "# A top-level comment\n",
        "\n",
        "[policy] # inline\n",
        "enforce = true\n",
        "\n",
    );
    let contract = Contract::parse(&complete(input))?;
    assert!(contract.enforce);
    assert!(contract.entries.is_empty());
    Ok(())
}

#[test]
fn a_hash_inside_a_quoted_value_survives() -> TestResult {
    let input = concat!(
        "[[approved]]\n",
        "crate = \"serde\"\n",
        "tier = \"boundary\"\n",
        "version = \"1.0\"\n",
        "reason = \"Derive-based serialization needs compiler introspection std does not expose.\"\n",
        "approved_by = \"reviewer\"\n",
        "approved_on = \"2026-08-19\"\n",
        "review = \"https://example.com/pr#123\"\n",
    );
    let contract = Contract::parse(&complete(input))?;
    assert_eq!(contract.entries[0].review, "https://example.com/pr#123");
    Ok(())
}

#[test]
fn an_unknown_key_is_refused_rather_than_skipped() {
    let input = entry("typo = \"boom\"\n");
    assert_eq!(
        Contract::parse(&input),
        Err(ContractError::UnknownKey {
            line: 15,
            key: "typo".into()
        })
    );
}

#[test]
fn a_key_before_any_section_is_refused() {
    let input = "orphan = \"value\"\n";
    assert_eq!(
        Contract::parse(&complete(input)),
        Err(ContractError::OrphanKey {
            line: 1,
            key: "orphan".into()
        })
    );
}

#[test]
fn a_missing_field_is_refused() {
    let input = concat!(
        "[[approved]]\n",
        "crate = \"serde\"\n",
        "tier = \"boundary\"\n",
        "version = \"1.0\"\n",
        "approved_by = \"reviewer\"\n",
        "approved_on = \"2026-08-19\"\n",
        "review = \"docs/ADMISSION.md\"\n",
    );
    assert_eq!(
        Contract::parse(&complete(input)),
        Err(ContractError::MissingField {
            krate: Some("serde".into()),
            field: "reason",
            line: 1
        })
    );
}

#[test]
fn an_eliminate_tier_entry_is_a_category_error() {
    let input = concat!(
        "[[approved]]\n",
        "crate = \"hex\"\n",
        "tier = \"eliminate\"\n",
        "version = \"0.4\"\n",
        "reason = \"Workspace stdlib replaces this; no external crate is admissible here.\"\n",
        "approved_by = \"reviewer\"\n",
        "approved_on = \"2026-08-19\"\n",
        "review = \"docs/ADMISSION.md\"\n",
    );
    assert_eq!(
        Contract::parse(&complete(input)),
        Err(ContractError::BadTier {
            line: 1,
            value: "eliminate".into()
        })
    );
}

#[test]
fn a_malformed_date_is_refused() {
    let input = concat!(
        "[[approved]]\n",
        "crate = \"serde\"\n",
        "tier = \"boundary\"\n",
        "version = \"1.0\"\n",
        "reason = \"Compiler introspection needed.\"\n",
        "approved_by = \"reviewer\"\n",
        "approved_on = \"19-08-2026\"\n",
        "review = \"docs/ADMISSION.md\"\n",
    );
    assert_eq!(
        Contract::parse(&complete(input)),
        Err(ContractError::BadDate {
            krate: "serde".into(),
            line: 13,
            value: "19-08-2026".into()
        })
    );
}

#[test]
fn a_reason_that_is_not_a_sentence_is_refused() {
    let input = concat!(
        "[[approved]]\n",
        "crate = \"serde\"\n",
        "tier = \"boundary\"\n",
        "version = \"1.0\"\n",
        "reason = \"needed\"\n",
        "approved_by = \"reviewer\"\n",
        "approved_on = \"2026-08-19\"\n",
        "review = \"docs/ADMISSION.md\"\n",
    );
    assert_eq!(
        Contract::parse(&complete(input)),
        Err(ContractError::ThinReason {
            krate: "serde".into()
        })
    );
}

#[test]
fn a_reason_that_restates_the_crate_name_is_refused() {
    let input = concat!(
        "[[approved]]\n",
        "crate = \"serde\"\n",
        "tier = \"boundary\"\n",
        "version = \"1.0\"\n",
        "reason = \"serde.\"\n",
        "approved_by = \"reviewer\"\n",
        "approved_on = \"2026-08-19\"\n",
        "review = \"docs/ADMISSION.md\"\n",
    );
    assert_eq!(
        Contract::parse(&complete(input)),
        Err(ContractError::ThinReason {
            krate: "serde".into()
        })
    );
}

#[test]
fn a_duplicate_approval_is_refused() {
    let input = format!("{}\n{}", entry(""), entry(""));
    assert_eq!(
        Contract::parse(&input),
        Err(ContractError::DuplicateEntry {
            krate: "serde".into(),
            line: 16
        })
    );
}

/// Lookup is byte-exact: an approval for one spelling does not admit a
/// package whose name is a `-`/`_` fold of it, and a caller that wants the
/// fold must author an explicit alias.
#[test]
fn lookup_is_exact_and_only_an_explicit_alias_is_tolerated() -> TestResult {
    let exact = Contract::parse(&entry(""))?;
    assert!(exact.approval_for("serde").is_some());
    assert!(
        exact.approval_for("ser-de").is_none(),
        "a fold-alike spelling must not borrow the approval"
    );
    assert!(
        exact.approval_for("SERDE").is_none(),
        "identity is case-sensitive, not folded"
    );

    let aliased = Contract::parse(&entry("aliases = \"ser-de\"\n"))?;
    assert!(
        aliased.approval_for("ser-de").is_some(),
        "an authored alias admits the observed spelling"
    );
    assert!(
        aliased.approval_for("serde").is_some(),
        "the canonical name stays admitted alongside its alias"
    );
    Ok(())
}

/// Two distinct spellings are two distinct approvals, not one — and a
/// compatibility alias a second package also needs is a collision.
#[test]
fn aliases_collide_rather_than_share_authority() -> TestResult {
    let distinct = Contract::parse(&format!(
        "{}\n{}",
        entry(""),
        entry("").replace("crate = \"serde\"", "crate = \"ser-de\"")
    ))?;
    assert_eq!(distinct.entry_count(), 2);
    assert!(distinct.approval_for("serde").is_some());
    assert!(distinct.approval_for("ser-de").is_some());

    let contested = Contract::parse(&format!(
        "{}\n{}",
        entry("aliases = \"other\"\n"),
        entry("").replace("crate = \"serde\"", "crate = \"other\"")
    ));
    assert!(
        matches!(contested, Err(ContractError::AliasCollision { ref alias, .. }) if alias == "other"),
        "an alias equal to another package's name is a collision"
    );

    let shared = Contract::parse(&format!(
        "{}\n{}",
        entry("aliases = \"shared\"\n"),
        entry("")
            .replace("crate = \"serde\"", "crate = \"alias_second\"")
            .replace(
                "review = \"docs/ADMISSION.md\"",
                "review = \"docs/ADMISSION.md\"\naliases = \"shared\""
            )
    ));
    assert!(
        matches!(shared, Err(ContractError::AliasCollision { ref alias, .. }) if alias == "shared"),
        "two packages may not share one alias"
    );
    Ok(())
}

/// An unsupported register schema is refused rather than read under the
/// older rules, and all three supported versions load (#788).
#[test]
fn an_unsupported_schema_is_refused() -> TestResult {
    assert!(matches!(
        Contract::parse("[policy]\nschema = 99\n"),
        Err(ContractError::UnsupportedSchema { line: 2, ref value }) if value == "99"
    ));
    assert!(matches!(
        Contract::parse("[policy]\nschema = \"2\"\n"),
        Err(ContractError::UnsupportedSchema { line: 2, .. })
    ));
    assert_eq!(Contract::parse("[policy]\nenforce = true\n")?.schema(), 1);
    assert_eq!(Contract::parse("[policy]\nschema = 2\n")?.schema(), 2);
    assert_eq!(Contract::parse("[policy]\nschema = 3\n")?.schema(), 3);
    Ok(())
}

/// Issue #208: `license` is a required field, and the register parser
/// refuses an expression it cannot read before the audit ever compares it
/// with Cargo. The accepted-*licence* question is the audit's; this is only
/// about the shape of the string.
#[test]
fn an_absent_license_field_is_refused() {
    let without = entry("").replace("license = \"MIT OR Apache-2.0\"\n", "");
    assert_eq!(
        Contract::parse(&without),
        Err(ContractError::MissingField {
            krate: Some("serde".into()),
            field: "license",
            line: 1,
        })
    );
}

/// Every spelling a human writes that is not an SPDX expression is refused
/// at the field, so a licence field cannot be a free-text note that silently
/// never matches what Cargo reports.
#[test]
fn an_unreadable_license_expression_is_refused_at_its_field() {
    for value in [
        "MIT OR",
        "OR MIT",
        "MIT AND",
        "MIT OR OR Apache-2.0",
        "MIT OR  Apache-2.0",
        "(MIT OR Apache-2.0",
        "MIT OR Apache-2.0)",
        "(MIT)(Apache-2.0)",
        "MIT or Apache-2.0",
        "MIT/Apache-2.0",
        "see LICENSE for terms",
        "MIT, Apache-2.0",
    ] {
        let input = entry("").replace(
            "license = \"MIT OR Apache-2.0\"",
            &format!("license = \"{value}\""),
        );
        assert!(
            matches!(
                Contract::parse(&input),
                Err(ContractError::InvalidField {
                    field: "license",
                    line: 7,
                    value: ref got,
                    ..
                }) if got == value
            ),
            "{value:?} is not an SPDX expression and must be refused, not recorded"
        );
    }
}

/// The positive half of the same grammar: every expression this repository
/// actually approves today, plus the parenthesised and `WITH` forms a
/// vendored licence statement can carry.
#[test]
fn a_readable_spdx_expression_is_kept_verbatim() -> TestResult {
    for value in [
        "MIT",
        "Apache-2.0",
        "MIT OR Apache-2.0",
        "CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception",
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
        "(MIT OR Apache-2.0) AND Unicode-3.0",
        "LicenseRef-Proprietary",
    ] {
        let input = entry("").replace(
            "license = \"MIT OR Apache-2.0\"",
            &format!("license = \"{value}\""),
        );
        let parsed = Contract::parse(&input)?;
        assert_eq!(parsed.entries[0].license(), value);
    }
    Ok(())
}
