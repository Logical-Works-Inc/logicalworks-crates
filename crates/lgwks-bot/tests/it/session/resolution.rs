//! How an answer is resolved: a semantic tier plugs in, a degraded resolver
//! re-asks without claiming absence, and an alias binds to the question it was
//! confirmed at.

use lgwks_bot::{
    Alias, DegradedReason, LanguageResolver, SemanticResolver, Session, TranscriptEntry, Value,
};

use super::*;

#[test]
fn a_session_resolves_a_phrase_only_a_model_can_relate() -> TestResult {
    // The claim this test exists to check is the integration one: a semantic
    // resolver is a `Resolver`, so it drops into `Session` with no change to the
    // session, the flow document, or the four verbs. The unit tests in
    // `semantic.rs` prove the tier's arithmetic; only this proves it plugs in.
    let embedder = OrderEmbedder {
        identity: EmbedderIdentity::new("test-model", "digest-test", 2)?,
        measurability: Measurability::Complete,
    };
    let mut session =
        Session::with_resolver("semantic", order_flow()?, SemanticResolver::new(embedder))?;

    // The premise, asserted rather than assumed: the default resolver cannot
    // reach this phrase. Without it the test could pass on a lexicon match and
    // prove nothing about the model.
    let mut default = Session::new("default", order_flow()?)?;
    default.answer("the usual")?;
    assert_eq!(
        default.current(),
        Some("ask"),
        "the lexicon alone must re-ask, or this test proves nothing about the model"
    );

    session.answer("the usual")?;

    assert_eq!(
        session.terminal(),
        Some(&Terminal::Completed),
        "the model's resolution advanced the session"
    );
    assert_eq!(
        session.scope().get("order"),
        Some(&Value::Choice(String::from("Repeat last order")))
    );
    Ok(())
}

#[test]
fn a_degraded_resolver_reasks_without_claiming_absence() -> TestResult {
    let session = Session::with_resolver(
        "degraded",
        one_question_flow()?,
        FixedResolver(Resolution::Degraded {
            reason: DegradedReason::EmbedderUnavailable,
        }),
    )?;
    let mut session = session;
    session.answer("yes")?;

    assert_eq!(
        session.current(),
        Some("ask"),
        "a degraded resolver re-asks the same node rather than advancing"
    );
    assert_eq!(session.terminal(), None);

    let roles: Vec<&str> = session
        .transcript()
        .iter()
        .map(TranscriptEntry::role)
        .collect();
    assert!(
        roles.contains(&"resolver-degraded"),
        "the cause is recorded under its own role, got {roles:?}"
    );
    let recorded = recorded_under(&session, "resolver-degraded");
    assert_eq!(
        recorded, "Resolver unavailable: the embedder is unavailable",
        "the record names the cause rather than reporting an empty score"
    );
    Ok(())
}

#[test]
fn an_unmeasurable_competitor_does_not_advance_the_session() -> TestResult {
    // End to end through the real `SemanticResolver`, not a fixed verdict. The
    // utterance and the competitor both embed to the zero vector: the width is
    // right, so the dimension check passes, and no angle can be drawn from
    // either. Before the repair this resolved to `Repeat last order` with a lead
    // equal to its whole score — a winner over a one-option field it was never
    // compared against — and the session advanced on it.
    let identity = || EmbedderIdentity::new("test-model", "digest-test", 2);
    let mut session = Session::with_resolver(
        "semantic",
        order_flow()?,
        SemanticResolver::new(OrderEmbedder {
            identity: identity()?,
            measurability: Measurability::DegenerateCompetitor,
        }),
    )?;

    session.answer("the usual")?;

    assert_eq!(
        session.current(),
        Some("ask"),
        "an incomplete comparison set must not advance the session"
    );
    assert_eq!(session.terminal(), None);
    assert_eq!(
        session.scope().get("order"),
        None,
        "no option was chosen, so the variable is not bound"
    );
    let recorded = recorded_under(&session, "resolver-degraded");
    assert_eq!(
        recorded, "Resolver unavailable: an embedding could not be measured",
        "the record names the unmeasurable embedding, not a missing dependency"
    );

    // The same flow with a healthy competitor set: one option is measured
    // against both competitors and the session completes. Without this half the
    // test could pass on a resolver that never advances at all.
    let mut healthy = Session::with_resolver(
        "semantic",
        order_flow()?,
        SemanticResolver::new(OrderEmbedder {
            identity: identity()?,
            measurability: Measurability::Complete,
        }),
    )?;
    healthy.answer("the usual")?;
    assert_eq!(
        healthy.terminal(),
        Some(&Terminal::Completed),
        "the same utterance resolves once every comparison can be made"
    );
    assert_eq!(
        healthy.scope().get("order"),
        Some(&Value::Choice(String::from("Repeat last order")))
    );
    Ok(())
}

#[test]
fn a_degraded_verdict_is_distinguishable_from_an_absent_one() -> TestResult {
    let mut absent = Session::with_resolver(
        "absent",
        one_question_flow()?,
        FixedResolver(Resolution::Absent { best_score: 0.0 }),
    )?;
    absent.answer("yes")?;
    let mut degraded = Session::with_resolver(
        "degraded",
        one_question_flow()?,
        FixedResolver(Resolution::Degraded {
            reason: DegradedReason::EmbedderUnavailable,
        }),
    )?;
    degraded.answer("yes")?;

    // Both re-ask, and that is the point: the *session* behaviour is the same,
    // while what is recorded is not. A two-valued verdict would leave these two
    // transcripts identical, and an operator reading a session that repeats the
    // same question would have no way to tell an unclear person from a resolver
    // that never ran.
    assert_eq!(
        degraded.transcript().len(),
        absent.transcript().len() + 1,
        "the degraded re-ask carries one record the absent one does not"
    );
    let absent_roles: Vec<&str> = absent
        .transcript()
        .iter()
        .map(TranscriptEntry::role)
        .collect();
    let degraded_roles: Vec<&str> = degraded
        .transcript()
        .iter()
        .map(TranscriptEntry::role)
        .collect();
    assert!(!absent_roles.contains(&"resolver-degraded"));
    assert!(degraded_roles.contains(&"resolver-degraded"));
    assert_ne!(absent_roles, degraded_roles);
    Ok(())
}

/// Routes every option to one target, which is all a routing test needs.
fn routes_to(options: &[String], target: &str) -> BTreeMap<String, String> {
    options
        .iter()
        .map(|option| (option.clone(), String::from(target)))
        .collect()
}

/// Two ask nodes with different vocabularies, the second reachable only by
/// answering the first.
///
/// The shape the multi-ask tests need: a phrase confirmed at `ask_one` has a
/// *different* candidate set at `ask_two`, and the person's answer at the first
/// question is what makes the second observable.
fn two_question_flow(first: &[&str], second: &[&str]) -> Result<FlowSpec, SessionError> {
    let first_options: Vec<String> = first.iter().map(|name| (*name).to_owned()).collect();
    let second_options: Vec<String> = second.iter().map(|name| (*name).to_owned()).collect();
    FlowSpec::new(
        BTreeMap::from([
            (
                String::from("first"),
                VarType::Choice(first_options.clone()),
            ),
            (
                String::from("second"),
                VarType::Choice(second_options.clone()),
            ),
        ]),
        "ask_one",
        BTreeMap::from([
            (
                String::from("ask_one"),
                NodeKind::Ask {
                    var: String::from("first"),
                    options: first_options.clone(),
                    routes: routes_to(&first_options, "ask_two"),
                },
            ),
            (
                String::from("ask_two"),
                NodeKind::Ask {
                    var: String::from("second"),
                    options: second_options.clone(),
                    routes: routes_to(&second_options, "done"),
                },
            ),
            (String::from("done"), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(8),
    )
}

/// Reads the single transcript record written under `role`, or an empty string.
///
/// Empty for a role nothing was recorded under, which is what the assertions that
/// compare against an empty string are about. A record that *is* there and holds
/// an empty text is indistinguishable from it here, so a caller that must tell the
/// two apart reads [`Session::transcript`] itself — which is why the two arms of
/// the ambiguity are spelled in this doc rather than left to a reader.
fn recorded_under(session: &Session, role: &str) -> String {
    match session
        .transcript()
        .iter()
        .find(|entry| entry.role() == role)
    {
        Some(entry) => entry.text().to_owned(),
        None => String::new(),
    }
}

/// The filed counterexample for #25, at session level: one resolver, two
/// questions. Before the fix, the same phrase selected index 0 at *both*, which
/// at the second question meant `Delete account` — an action nobody confirmed.
#[test]
fn an_alias_confirmed_at_one_ask_cannot_answer_another() -> TestResult {
    let resolver = LanguageResolver::with_aliases(vec![Alias::new(
        "ask_one",
        "the usual",
        "Repeat last order",
    )]);
    let mut session = Session::with_resolver(
        "scope",
        two_question_flow(
            &["Repeat last order", "Cancel"],
            &["Delete account", "Keep account"],
        )?,
        resolver,
    )?;

    session.answer("the usual")?;
    assert_eq!(
        session.scope().get("first"),
        Some(&Value::Choice(String::from("Repeat last order"))),
        "the confirmation is honoured at the question it was made in"
    );
    assert_eq!(session.current(), Some("ask_two"));

    session.answer("the usual")?;
    assert_eq!(
        session.current(),
        Some("ask_two"),
        "the second question must not consume the first question's confirmation"
    );
    assert_eq!(
        session.scope().get("second"),
        None,
        "and must certainly not select the option sitting at its old index"
    );
    Ok(())
}

/// The filed counterexample for #25, second half: the bound option is removed
/// and its successor is the dangerous one. Teaching row 0 and then replacing row
/// 0's text must not transfer the confirmation to whatever moved in.
#[test]
fn a_confirmation_whose_option_was_removed_reasks_instead_of_selecting_its_successor() -> TestResult
{
    let resolver = LanguageResolver::with_aliases(vec![Alias::new(
        "ask_one",
        "the usual",
        "Repeat last order",
    )]);
    // Version two of the same question: `Repeat last order` is gone and
    // `Delete account` now sits where it was.
    let mut session = Session::with_resolver(
        "stale",
        two_question_flow(&["Delete account", "Cancel"], &["Yes", "No"])?,
        resolver,
    )?;

    session.answer("the usual")?;

    assert_eq!(
        session.current(),
        Some("ask_one"),
        "a withdrawn confirmation re-asks rather than advancing"
    );
    assert_eq!(
        session.scope().get("first"),
        None,
        "no value is stored for an answer the resolver refused to read"
    );
    assert_eq!(
        recorded_under(&session, "resolver-stale-alias"),
        "Superseded alias: question ask_one no longer offers \"Repeat last order\"",
        "the record names both halves of the broken binding, because either \
         alone is unactionable"
    );

    // The person can still answer: the withdrawal is not a trap, it is a
    // withdrawal. Typing without the withdrawn phrase resolves normally.
    session.answer("Cancel")?;
    assert_eq!(
        session.scope().get("first"),
        Some(&Value::Choice(String::from("Cancel")))
    );
    Ok(())
}
