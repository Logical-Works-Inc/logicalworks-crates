//! The ceilings a document and an operator set: a document may tighten its own
//! and may not raise them, and the aggregate ceiling bounds a looping
//! conversation (issue 40).

use lgwks_bot::{
    LanguageResolver, MAX_SESSION_BYTES, MAX_UTTERANCE_BYTES, ResourceAxis, ResourceLimits, Session,
};

use super::limits::{retained_recorded_bytes, speak_back_flow};
use super::*;

/// A JSON document for [`speak_back_flow`], carrying the supplied `bounds`.
///
/// Written as text rather than serialized so the test reaches the parser a
/// caller reaches — [`FlowSpec::from_json`] — on a document shaped like a
/// caller's. Only the template and the candidate list are interpolated, and a
/// caller of this helper passes strings with no characters the format has to
/// escape.
fn speak_back_document(say_text: &str, options: &[String], bounds: &str) -> String {
    let quoted = options
        .iter()
        .map(|option| format!("\"{option}\""))
        .collect::<Vec<String>>()
        .join(", ");
    let routes = options
        .iter()
        .map(|option| format!("\"{option}\": \"say\""))
        .collect::<Vec<String>>()
        .join(", ");
    format!(
        "{{\"vars\": {{\"answer\": \"string\"}}, \
         \"entry\": \"ask\", \
         \"nodes\": {{ \
         \"ask\": {{\"ask\": {{\"var\": \"answer\", \"options\": [{quoted}], \
         \"routes\": {{{routes}}}}}}}, \
         \"say\": {{\"say\": {{\"text\": \"{say_text}\"}}}}, \
         \"done\": \"end\"}}, \
         \"edges\": [{{\"next\": {{\"from\": \"say\", \"to\": \"done\"}}}}], \
         \"bounds\": {bounds}}}"
    )
}

/// A flow whose ask loops back through a say node until the person stops.
///
/// The repetition the aggregate ceiling exists for: each turn is a handful of
/// individually acceptable records, and the sum of them is what needs a bound.
fn loop_flow() -> Result<FlowSpec, SessionError> {
    FlowSpec::new(
        BTreeMap::from([(String::from("answer"), VarType::String)]),
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                ask(
                    "answer",
                    &["again", "stop"],
                    &[("again", "say"), ("stop", "done")],
                ),
            ),
            (String::from("say"), say("tick")),
            (String::from("done"), NodeKind::End),
        ]),
        vec![lgwks_bot::FlowEdge::next("say", "ask")],
        BTreeMap::new(),
        FlowBounds::new(64),
    )
}

#[test]
fn a_document_may_tighten_its_own_ceilings_and_may_not_raise_them() -> TestResult {
    // The public path: JSON document, validated flow, session.
    let modest = speak_back_document(
        "${answer}",
        &[String::from("yes")],
        "{\"budget\": 16, \"resources\": {\"record_bytes\": 1024}}",
    );
    let flow = FlowSpec::from_json(&modest)?;
    assert_eq!(
        flow.bounds()
            .resources()
            .map(|limits| limits.get(ResourceAxis::Record)),
        Some(1_024),
        "the document's requested ceiling survives parsing"
    );
    let session = Session::new("modest", flow)?;
    assert_eq!(
        session.limits().get(ResourceAxis::Record),
        1_024,
        "the session enforces the narrower of the document's request and the operator's ceiling"
    );
    assert_eq!(
        session.limits().get(ResourceAxis::Utterance),
        MAX_UTTERANCE_BYTES,
        "an axis the document did not name keeps the operator's ceiling"
    );

    let greedy = speak_back_document(
        "${answer}",
        &[String::from("yes")],
        "{\"budget\": 16, \"resources\": {\"session_bytes\": 4294967296}}",
    );
    match FlowSpec::from_json(&greedy) {
        Err(SessionError::ResourceLimitAboveCeiling {
            axis,
            requested,
            ceiling,
        }) => {
            assert_eq!(axis, ResourceAxis::Session, "the refusal names the axis");
            assert_eq!(
                requested, 4_294_967_296,
                "the ceiling the document asked for"
            );
            assert_eq!(
                ceiling, MAX_SESSION_BYTES,
                "the operator's hard ceiling, unchanged by the request"
            );
        }
        other => {
            return Err(format!("expected the ceiling refusal at load, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn a_configured_ceiling_above_the_operators_is_refused_rather_than_clamped() -> TestResult {
    // A bound that is silently ignored is a bound nobody can reason about, and
    // an operator that asked for no ceiling must not run believing it has one.
    let flow = speak_back_flow("${answer}", &[String::from("x")])?;
    let unbounded = ResourceLimits::shipped().tighten(ResourceAxis::Session, usize::MAX);
    let refusal = Session::with_limits("unbounded", flow, unbounded)
        .err()
        .ok_or("a ceiling above the operator's was accepted")?;
    match refusal {
        SessionError::ResourceLimitAboveCeiling {
            axis,
            requested,
            ceiling,
        } => {
            assert_eq!(axis, ResourceAxis::Session, "the refusal names the axis");
            assert_eq!(requested, usize::MAX, "the ceiling that was asked for");
            assert_eq!(ceiling, MAX_SESSION_BYTES, "the operator's hard ceiling");
        }
        other => {
            return Err(format!("expected the ceiling refusal, got {other:?}").into());
        }
    }
    // And an operator ceiling below the document's request wins, per axis.
    let document = speak_back_document(
        "${answer}",
        &[String::from("x")],
        "{\"budget\": 16, \"resources\": {\"record_bytes\": 1024}}",
    );
    let limits = ResourceLimits::shipped().tighten(ResourceAxis::Record, 64);
    let session = Session::with_limits("narrow", FlowSpec::from_json(&document)?, limits)?;
    assert_eq!(
        session.limits().get(ResourceAxis::Record),
        64,
        "the operator's smaller ceiling is the one in force"
    );
    Ok(())
}

#[test]
fn cumulative_records_are_charged_against_the_session_ceiling() -> TestResult {
    // Every turn of this loop is a handful of individually acceptable records.
    // The per-record ceiling never refuses one; the aggregate is what bounds
    // the conversation.
    let (journal, records) = SharedJournal::new();
    let mut session = Session::with_components_and_limits(
        "generous",
        loop_flow()?,
        LanguageResolver::new(),
        journal,
        ResourceLimits::shipped(),
    )?;
    session.answer("again")?;
    session.answer("again")?;
    session.answer("stop")?;
    let total = session.retained_bytes();
    let full = records.borrow().clone();
    assert_eq!(
        total,
        retained_recorded_bytes(&session),
        "the retained counter is the sum of the records and the path"
    );
    assert!(
        session
            .transcript()
            .iter()
            .filter(|entry| entry.role() == "assistant")
            .count()
            > 1,
        "the loop recorded repeated prompts, which is what accumulates"
    );

    // A ceiling of exactly that total admits the whole script: the boundary
    // from the accepting side.
    let exact = ResourceLimits::shipped().tighten(ResourceAxis::Session, total);
    let (journal, _) = SharedJournal::new();
    let mut session = Session::with_components_and_limits(
        "exact",
        loop_flow()?,
        LanguageResolver::new(),
        journal,
        exact,
    )?;
    session.answer("again")?;
    session.answer("again")?;
    session.answer("stop")?;
    assert_eq!(
        session.retained_bytes(),
        total,
        "the script that fits exactly is run to completion"
    );
    assert_eq!(
        session.terminal(),
        Some(&Terminal::Completed),
        "and reaches its terminal"
    );

    // Half of it does not, and what was written before the refusal is a prefix
    // of what the full run wrote: no partial record, no hole.
    let half = total
        .checked_div(2)
        .ok_or("half of a measured positive total is never zero")?;
    assert!(
        half > 0 && half < total,
        "the fixture's half-ceiling ({half}) is a genuine tightening of {total}"
    );
    let (journal, records) = SharedJournal::new();
    let mut session = match Session::with_components_and_limits(
        "half",
        loop_flow()?,
        LanguageResolver::new(),
        journal,
        ResourceLimits::shipped().tighten(ResourceAxis::Session, half),
    ) {
        Ok(session) => session,
        // Construction records the opening prompt, so a half-ceiling below that
        // one record would fail before the loop this test is about. That is a
        // defect in the fixture's arithmetic, and it says so rather than
        // reporting a refusal that the bound did not cause.
        Err(SessionError::SessionRetentionExceeded { .. }) => {
            return Err(
                "the fixture's first record does not fit half of the run's own total; the \
                 fixture is wrong, not the bound"
                    .into(),
            );
        }
        Err(other) => return Err(format!("expected a session, got {other:?}").into()),
    };
    let mut refusal = None;
    for _attempt in 0..8 {
        if let Err(error) = session.answer("again") {
            refusal = Some(error);
            break;
        }
    }
    let Some(SessionError::SessionRetentionExceeded { bytes, limit }) = refusal else {
        return Err(format!("expected the retention refusal, got {refusal:?}").into());
    };
    assert_eq!(limit, half, "the refusal reports the ceiling in force");
    assert!(
        bytes > limit,
        "the refusal reports the total the write would have reached ({bytes}), which is past \
         the {limit}-byte ceiling it was checked against"
    );
    assert!(
        session.retained_bytes() <= half,
        "nothing past the ceiling was retained: {} stayed within {half}",
        session.retained_bytes()
    );
    let written = records.borrow();
    assert!(
        written.len() < full.len(),
        "the run that hit the ceiling wrote fewer records ({}) than the run that did not ({})",
        written.len(),
        full.len()
    );
    assert_eq!(
        written.as_slice(),
        &full[..written.len()],
        "and what it wrote is a prefix of the full run, with no partial record at the end"
    );
    drop(written);
    assert_eq!(
        session.retained_bytes(),
        retained_recorded_bytes(&session),
        "the counter and the retained records agree after the refusal"
    );
    Ok(())
}

#[test]
fn a_session_that_cannot_retain_its_first_record_writes_nothing() -> TestResult {
    // The construction path, where the aggregate ceiling is hit before any
    // conversation exists. The refusal is observable through the journal rather
    // than only asserted, because the error path returns no session to inspect.
    let (journal, records) = SharedJournal::new();
    let result = Session::with_components_and_limits(
        "tiny",
        loop_flow()?,
        LanguageResolver::new(),
        journal,
        ResourceLimits::shipped().tighten(ResourceAxis::Session, 8),
    );
    match result {
        Err(SessionError::SessionRetentionExceeded { bytes, limit }) => {
            assert!(
                bytes > limit,
                "the refusal reports a total past a ceiling of {limit}"
            );
        }
        Ok(_session) => {
            return Err("expected the retention refusal, got a session".into());
        }
        Err(other) => {
            return Err(format!("expected the retention refusal, got {other:?}").into());
        }
    }
    assert!(
        records.borrow().is_empty(),
        "a session refused during construction wrote no record at all"
    );
    Ok(())
}
