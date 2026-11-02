//! The `[policy]` block's enforcement switch, read under a closed grammar.

use super::*;

#[test]
fn enforcement_defaults_to_on_when_no_policy_is_written() -> TestResult {
    let contract = Contract::parse(&entry(""))?;
    assert!(contract.enforce);
    Ok(())
}

#[test]
fn policy_can_stand_enforcement_down_for_adoption() -> TestResult {
    let input = format!("[policy]\nenforce = false\n\n{}", entry(""));
    let contract = Contract::parse(&input)?;
    assert!(!contract.enforce);
    Ok(())
}

/// The exact spellings the closed grammar admits. `true` and `false` are
/// the only two tokens; everything else is refused below.
#[test]
fn the_boolean_grammar_admits_exactly_true_and_false() -> TestResult {
    let on = Contract::parse("[policy]\nenforce = true\n")?;
    assert!(on.enforce, "`enforce = true` must leave enforcement on");
    let off = Contract::parse("[policy]\nenforce = false\n")?;
    assert!(!off.enforce, "`enforce = false` is adoption mode");
    Ok(())
}

/// Issue 45's three literal inputs plus the neighbouring spellings a human
/// actually writes. Each one used to be read as `false`, standing the gate
/// down without a diagnostic; each must now be a typed refusal that names
/// the line.
#[test]
fn a_malformed_enforce_value_is_refused_rather_than_read_as_false() {
    // Surrounding whitespace is the reader's, not the value's: `split_pair`
    // trims it, so `enforce =   true` is the legal spelling with padding and
    // is covered by the positive test above.
    for token in [
        "True",
        "FALSE",
        "\"true\"",
        "\"false\"",
        "1",
        "0",
        "yes",
        "on",
        "",
        "true.",
        "true;",
    ] {
        let input = format!("[policy]\nenforce = {token}\n");
        assert_eq!(
            Contract::parse(&input),
            Err(ContractError::BadPolicyValue {
                line: 2,
                key: "enforce".into(),
                value: token.into(),
            }),
            "`enforce = {token}` must be refused, not silently read as false"
        );
    }
}

/// A trailing comment is stripped before the token is read, so the two
/// legal spellings keep working with one written beside them.
#[test]
fn a_trailing_comment_does_not_change_the_policy_token() -> TestResult {
    let contract = Contract::parse("[policy]\nenforce = false # adoption\n")?;
    assert!(!contract.enforce);
    Ok(())
}

#[test]
fn a_repeated_policy_key_is_refused() {
    assert_eq!(
        Contract::parse("[policy]\nenforce = true\nenforce = false\n"),
        Err(ContractError::DuplicatePolicyKey {
            line: 3,
            key: "enforce".into(),
        })
    );
}

#[test]
fn a_repeated_policy_section_is_refused() {
    assert_eq!(
        Contract::parse("[policy]\nenforce = true\n[policy]\nenforce = false\n"),
        Err(ContractError::DuplicatePolicySection { line: 3 })
    );
}

#[test]
fn a_policy_block_after_the_entries_is_still_read() -> TestResult {
    let input = format!("{}\n[policy]\nenforce = false\n", entry(""));
    let contract = Contract::parse(&input)?;
    assert!(!contract.enforce);
    Ok(())
}
