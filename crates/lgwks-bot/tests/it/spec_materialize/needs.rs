//! T23: one complete `NeedSet`, and nothing runs.

use lgwks_bot::{Admission, Cap, GrantSet, Need};

use super::{TestResult, admit, clear_trace, scope, take_trace};

#[test]
fn admission_reports_all_five_needs_in_one_set() -> TestResult<()> {
    clear_trace();
    let refusal = admit(
        r#"{"version":1,"name":"needs","chains":[
            {"source":"unknown::one","target":"x",
             "on":[["always",{"domain":"test::page","target":"a"}]]},
            {"source":"unknown::two","target":"x",
             "on":[["always",{"domain":"test::page","target":"a"}]]},
            {"source":"test::counter","target":"3",
             "on":[["always",{"domain":"unknown::action","target":"a"}]]},
            {"source":"test::counter","target":"not-a-count",
             "on":[["always",{"domain":"test::page","target":"a"}]]},
            {"source":"test::net_counter","target":"3",
             "on":[["always",{"domain":"test::page","target":"a"}]]}]}"#,
        &GrantSet::empty(),
        scope()?,
    );

    let Err(Admission::Needs(needs)) = refusal else {
        return Err(format!("expected a NeedSet, got {refusal:?}").into());
    };

    assert_eq!(
        needs.needs(),
        [
            Need::UnknownSource {
                chain: 0,
                domain: "unknown::one".into(),
            },
            Need::UnknownSource {
                chain: 1,
                domain: "unknown::two".into(),
            },
            Need::UnknownAction {
                chain: 2,
                action: 0,
                domain: "unknown::action".into(),
            },
            Need::SourceTargetRejected {
                chain: 3,
                domain: "test::counter".into(),
                // Both halves, because the constructor reports both: the field
                // that is missing and the parse failure that made it missing.
                cause: "incomplete bot spec: missing target: invalid digit found in string".into(),
            },
            Need::MissingCapability {
                chain: 4,
                action: None,
                domain: "test::net_counter".into(),
                capability: Cap::net(),
            },
        ],
        "one refusal must carry all five needs, attributed and in declaration order"
    );
    assert_eq!(
        needs.len(),
        5,
        "no more and no fewer than the five injected"
    );

    assert!(
        take_trace().is_empty(),
        "a refused materialization must poll nothing and execute nothing"
    );
    Ok(())
}

#[test]
fn proposed_grants_closes_the_capability_need_without_granting() -> TestResult<()> {
    let json = r#"{"version":1,"name":"needs","chains":[
        {"source":"test::net_counter","target":"3",
         "on":[["always",{"domain":"test::net_page","target":"a"}]]}]}"#;
    let refusal = admit(json, &GrantSet::empty(), scope()?);
    let Err(Admission::Needs(needs)) = refusal else {
        return Err(format!("expected a NeedSet, got {refusal:?}").into());
    };
    assert_eq!(needs.len(), 2, "source and action each need `bot.net`");

    let proposal = needs.proposed_grants();
    assert!(
        proposal.grants(&Cap::net()),
        "the proposal must name the capability that closes the need"
    );
    // The proposal is data; the materializer still refuses the same spec, so it
    // was never a grant.
    let still = admit(json, &GrantSet::empty(), scope()?);
    assert!(
        matches!(still, Err(Admission::Needs(_))),
        "a repair proposal must not admit the bot on its own: {still:?}"
    );
    Ok(())
}

#[test]
fn unknown_condition_is_a_need_not_an_inert_gate() -> TestResult<()> {
    let refusal = admit(
        r#"{"version":1,"name":"cond","chains":[
            {"source":"test::counter","target":"3",
             "on":[["mystery::gate",{"domain":"test::page","target":"a"}]]}]}"#,
        &GrantSet::empty(),
        scope()?,
    );
    let Err(Admission::Needs(needs)) = refusal else {
        return Err(format!("expected a NeedSet, got {refusal:?}").into());
    };
    assert_eq!(
        needs.needs(),
        [Need::UnknownCondition {
            chain: 0,
            action: 0,
            condition: "mystery::gate".into(),
        }],
        "an unknown condition must be reported, never treated as always-true"
    );
    Ok(())
}

#[test]
fn a_spec_cannot_grant_itself_authority() -> TestResult<()> {
    // The spec names a `bot.net` source, but the caller holds no grant. The
    // source's identifier buys no reach.
    let refusal = admit(
        r#"{"version":1,"name":"self-grant","chains":[
            {"source":"test::net_counter","target":"3",
             "on":[["always",{"domain":"test::page","target":"a"}]]}]}"#,
        &GrantSet::empty(),
        scope()?,
    );
    assert!(
        matches!(refusal, Err(Admission::Needs(_))),
        "naming a domain must not grant the capability it needs: {refusal:?}"
    );
    Ok(())
}
