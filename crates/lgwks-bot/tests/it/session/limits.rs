//! Issue 40: the byte ceilings a flow's shape does not bound.
//!
//! Validation bounds a document's *shape* and `FlowBounds` bounds its *steps*.
//! Neither bounds bytes, and the two are independent: `${answer}` written forty
//! thousand times is a two-hundred-kilobyte document that interpolates to a
//! hundred and sixty megabytes, and one `say` node is one step however much
//! text it renders. The tests below use ceilings small enough to run in a test
//! binary and pin the byte arithmetic a shipped ceiling applies to the same
//! shape, so the amplification is exercised rather than described.

use lgwks_bot::language::LanguageResolver;
use lgwks_bot::session::{
    FlowEdge, MAX_RECORD_BYTES, MAX_UTTERANCE_BYTES, MAX_VALUE_BYTES, ResourceAxis, ResourceLimits,
    Session, Value,
};

use super::*;

/// A flow that asks `answer` once, speaks `say_text`, and ends.
///
/// The shape the amplification needs: the spoken text is built from an answer
/// the person supplies, so the document can be small while the expansion is
/// not.
pub(super) fn speak_back_flow(
    say_text: &str,
    options: &[String],
) -> Result<FlowSpec, SessionError> {
    let routes = options
        .iter()
        .map(|option| (option.clone(), String::from("say")))
        .collect();
    FlowSpec::new(
        BTreeMap::from([(String::from("answer"), VarType::String)]),
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                NodeKind::Ask {
                    var: String::from("answer"),
                    options: options.to_vec(),
                    routes,
                },
            ),
            (
                String::from("say"),
                NodeKind::Say {
                    text: say_text.to_owned(),
                },
            ),
            (String::from("done"), NodeKind::End),
        ]),
        vec![FlowEdge::next("say", "done")],
        BTreeMap::new(),
        FlowBounds::new(16),
    )
}

/// The bytes a session retains, recounted from its public surface.
///
/// Written out here rather than read from `Session::retained_bytes` so the
/// assertion compares the counter to what the session actually holds: a
/// comparison of the counter with itself would agree with itself whatever it
/// counted.
pub(super) fn retained_recorded_bytes(session: &Session) -> usize {
    let records = session.transcript().iter().fold(0usize, |total, entry| {
        total
            .saturating_add(entry.text().len())
            .saturating_add(entry.path_node().len())
            .saturating_add(entry.role().len())
    });
    session
        .visited()
        .iter()
        .fold(records, |total, node| total.saturating_add(node.len()))
}

#[test]
fn a_placeholder_is_charged_the_bytes_it_expands_to() {
    // Multibyte text is charged its UTF-8 bytes, not its characters: the
    // expansion pastes bytes into a buffer, and a ceiling counted in characters
    // would let a document four times its own limit through.
    let multibyte = Value::String("\u{e9}".repeat(512));
    assert_eq!(
        multibyte.rendered_bytes(),
        1024,
        "512 two-byte characters occupy 1024 bytes"
    );
    assert_eq!(
        "\u{e9}".repeat(512).chars().count(),
        512,
        "the character count is half the byte count, which is why the bytes are what is charged"
    );
    // Integers are charged the width of their decimal spelling — what
    // interpolation writes — and not the width of the candidate that produced
    // them.
    assert_eq!(
        Value::Integer(0).rendered_bytes(),
        1,
        "zero renders as one digit"
    );
    assert_eq!(
        Value::Integer(-7).rendered_bytes(),
        2,
        "a negative integer renders its sign"
    );
    assert_eq!(
        Value::Integer(12_345).rendered_bytes(),
        5,
        "five digits render five bytes"
    );
    assert_eq!(
        Value::Integer(i64::MAX).rendered_bytes(),
        19,
        "the widest positive integer is nineteen bytes"
    );
    assert_eq!(
        Value::Integer(i64::MIN).rendered_bytes(),
        20,
        "the widest negative integer is twenty, and its magnitude has no positive counterpart"
    );
    assert_eq!(
        Value::Boolean(true).rendered_bytes(),
        4,
        "true renders as four bytes"
    );
    assert_eq!(
        Value::Boolean(false).rendered_bytes(),
        5,
        "false renders as five bytes"
    );
}

#[test]
fn a_repeated_placeholder_is_refused_at_its_computed_size() -> TestResult {
    // The report's defect at a scale a test binary can afford. Twenty thousand
    // repetitions of a value at the shipped utterance ceiling is a two-hundred-
    // kilobyte document whose expansion is a hundred and sixty-three megabytes:
    // the document validates, the budget of sixteen steps is untouched, and the
    // refusal reports the size of a string that was never built. The value is
    // as large as the ingress ceiling allows, which is the most a document can
    // get out of one answer and therefore the worst case for this shape.
    let value = "z".repeat(MAX_UTTERANCE_BYTES);
    let repeats = 20_000usize;
    let template = "${answer}".repeat(repeats);
    let expansion = repeats
        .checked_mul(value.len())
        .ok_or("the fixture's expansion overflows usize")?;
    // Above the prompt's own size, which carries the candidate in full, and far
    // below the expansion the template asks for.
    let ceiling = 16_384usize;
    assert!(
        expansion > MAX_RECORD_BYTES,
        "the fixture's expansion ({expansion} bytes) is over the shipped record ceiling too, \
         so the contraction is not an artefact of a deliberately tiny bound"
    );
    let limits = ResourceLimits::shipped().tighten(ResourceAxis::Record, ceiling);
    let (journal, records) = SharedJournal::new();
    let mut session = Session::with_components_and_limits(
        "amplify",
        speak_back_flow(&template, std::slice::from_ref(&value))?,
        LanguageResolver::new(),
        journal,
        limits,
    )?;
    let written_before = records.borrow().len();
    let retained_before = session.retained_bytes();
    assert_eq!(
        session.limits().get(ResourceAxis::Record),
        ceiling,
        "the session enforces the tightened ceiling"
    );
    match session.answer(&value) {
        Err(SessionError::TemplateExpansionTooLarge { node, bytes, limit }) => {
            assert_eq!(node, "say", "the refusal names the template's node");
            assert_eq!(
                bytes, expansion,
                "the refusal reports the exact expansion, computed rather than measured: \
                 164 MB from a 200 KB document"
            );
            assert_eq!(limit, ceiling, "the refusal reports the ceiling in force");
        }
        other => {
            return Err(format!("expected the expansion refusal, got {other:?}").into());
        }
    }
    assert!(
        records.borrow().len() > written_before,
        "the answer's own record was written before the say node was reached"
    );
    assert_eq!(
        records.borrow().len(),
        session.transcript().len(),
        "the journal and the transcript hold the same records: the refused write left no \
         partial record in either"
    );
    assert!(
        records.borrow().iter().all(|record| record.0 != "say"),
        "the refused say record was never handed to the journal"
    );
    assert_eq!(
        session.current(),
        Some("say"),
        "the session is left at the node whose text it could not speak"
    );
    assert!(
        session.visited().iter().any(|node| node == "say"),
        "the say node was entered, charged, and then refused its own text"
    );
    assert_eq!(
        session.retained_bytes(),
        retained_recorded_bytes(&session),
        "the retained counter is the sum of what the session holds"
    );
    assert!(
        session.retained_bytes() > retained_before,
        "the accepted part of the answer is retained"
    );
    assert_eq!(
        session.terminal(),
        None,
        "a refused expansion does not end the run: the session is not completed, \
         it is stuck at the node whose text it could not speak"
    );
    Ok(())
}

#[test]
fn an_expansion_exactly_at_the_ceiling_succeeds_and_one_byte_over_is_refused() -> TestResult {
    // The boundary, from both sides. Sixteen repetitions of a sixty-four-byte
    // value is exactly the ceiling; seventeen is one byte past it.
    let value = "z".repeat(64);
    let repeats = 16usize;
    let ceiling = 1_024usize;
    assert_eq!(
        repeats.checked_mul(value.len()),
        Some(ceiling),
        "the fixture fills the ceiling exactly"
    );
    let limits = ResourceLimits::shipped().tighten(ResourceAxis::Record, ceiling);
    let fits = speak_back_flow(&"${answer}".repeat(repeats), std::slice::from_ref(&value))?;
    let mut session = Session::with_limits("fits", fits, limits)?;
    session.answer(&value)?;
    let spoken = session
        .transcript()
        .iter()
        .find(|entry| entry.path_node() == "say")
        .ok_or("the say node recorded the text it spoke")?;
    assert_eq!(
        spoken.text().len(),
        ceiling,
        "the record fills the ceiling exactly and is accepted"
    );
    assert_eq!(
        session.terminal(),
        Some(&Terminal::Completed),
        "a record of exactly the ceiling is spoken and the flow completes"
    );

    let over = repeats
        .checked_add(1)
        .ok_or("the fixture's repeat count overflows")?;
    let refused_bytes = over
        .checked_mul(value.len())
        .ok_or("the fixture's expansion overflows usize")?;
    let (journal, records) = SharedJournal::new();
    let mut session = Session::with_components_and_limits(
        "over",
        speak_back_flow(&"${answer}".repeat(over), std::slice::from_ref(&value))?,
        LanguageResolver::new(),
        journal,
        limits,
    )?;
    match session.answer(&value) {
        Err(SessionError::TemplateExpansionTooLarge { node, bytes, limit }) => {
            assert_eq!(node, "say", "the refusal names the template's node");
            assert_eq!(bytes, refused_bytes, "one byte over the ceiling");
            assert_eq!(limit, ceiling, "the refusal reports the ceiling in force");
        }
        other => {
            return Err(format!("expected the expansion refusal, got {other:?}").into());
        }
    }
    assert!(
        records.borrow().iter().all(|record| record.0 != "say"),
        "the refused record was never handed to the journal"
    );
    assert!(
        session
            .transcript()
            .iter()
            .all(|entry| entry.path_node() != "say"),
        "and it is not in the transcript either"
    );
    assert_eq!(
        session.retained_bytes(),
        retained_recorded_bytes(&session),
        "the refused expansion retained nothing the session does not hold"
    );
    Ok(())
}

#[test]
fn an_utterance_at_the_ceiling_is_accepted_and_one_byte_over_is_refused_before_anything_is_written()
-> TestResult {
    let ceiling = 8usize;
    let limits = ResourceLimits::shipped().tighten(ResourceAxis::Utterance, ceiling);
    let option = String::from("x");
    let at_ceiling = "12345678";
    assert_eq!(
        at_ceiling.len(),
        ceiling,
        "the fixture is exactly the ceiling"
    );

    let (journal, records) = SharedJournal::new();
    let mut session = Session::with_components_and_limits(
        "at",
        speak_back_flow("${answer}", std::slice::from_ref(&option))?,
        LanguageResolver::new(),
        journal,
        limits,
    )?;
    let written_before = records.borrow().len();
    // Not the option, so the ask re-asks: the point is that the size ceiling
    // did not refuse it.
    session.answer(at_ceiling)?;
    assert!(
        records.borrow().len() > written_before,
        "an utterance of exactly the ceiling reaches the conversation"
    );

    let (journal, records) = SharedJournal::new();
    let mut session = Session::with_components_and_limits(
        "over",
        speak_back_flow("${answer}", std::slice::from_ref(&option))?,
        LanguageResolver::new(),
        journal,
        limits,
    )?;
    let written_before = records.borrow().len();
    let steps_before = session.steps();
    let retained_before = session.retained_bytes();
    let one_over = "123456789";
    match session.answer(one_over) {
        Err(SessionError::UtteranceTooLarge { bytes, limit }) => {
            assert_eq!(
                bytes,
                one_over.len(),
                "the refusal counts the bytes offered"
            );
            assert_eq!(limit, ceiling, "the refusal reports the ceiling in force");
        }
        other => {
            return Err(format!("expected the utterance refusal, got {other:?}").into());
        }
    }
    assert_eq!(
        records.borrow().len(),
        written_before,
        "the refused utterance wrote no record"
    );
    assert_eq!(
        session.steps(),
        steps_before,
        "and charged no step: the ingress ceiling is checked before the step is charged"
    );
    assert_eq!(
        session.retained_bytes(),
        retained_before,
        "and retained nothing, so it is not in the transcript"
    );
    Ok(())
}

#[test]
fn an_ask_candidate_too_wide_for_the_session_is_refused_at_load() -> TestResult {
    // The load-time half, and the reason it exists: the candidate list and the
    // applied ceiling are both known while the document loads, so an ask whose
    // answer could never be stored is refused there rather than after the
    // person has typed it.
    let option = "x".repeat(MAX_VALUE_BYTES);
    let at_ceiling = single_ask_flow(VarType::String, vec![option.clone()])?;
    Session::with_limits("at", at_ceiling, ResourceLimits::shipped())?;

    // One byte over, and it is the document that refuses it: the flow never
    // becomes a session, so the candidate is refused before anyone is asked it.
    let one_over = "y".repeat(MAX_VALUE_BYTES.saturating_add(1));
    let refusal = single_ask_flow(VarType::String, vec![one_over.clone()])
        .err()
        .ok_or("a document offering a candidate wider than the shipped ceiling loaded")?;
    match refusal {
        SessionError::AskOptionTooLarge {
            node,
            variable,
            option: offered,
            bytes,
            limit,
        } => {
            assert_eq!(node, "ask", "the refusal names the ask node");
            assert_eq!(
                variable, "answer",
                "the refusal names the variable the ask writes"
            );
            assert_eq!(offered, one_over, "the refusal names the candidate");
            assert_eq!(bytes, one_over.len(), "one byte over the shipped ceiling");
            assert_eq!(limit, MAX_VALUE_BYTES, "the shipped value ceiling");
        }
        other => {
            return Err(format!("expected the candidate refusal, got {other:?}").into());
        }
    }

    // A tighter session ceiling refuses a candidate the shipped one accepts.
    // The document is well-formed; it is the session that cannot run it, and
    // that is why the check runs a second time inside the constructor rather
    // than only where the document was parsed.
    let narrow = single_ask_flow(VarType::String, vec![option.clone()])?;
    let tightened = ResourceLimits::shipped().tighten(ResourceAxis::Value, 32);
    let refusal = Session::with_limits("narrow", narrow, tightened)
        .err()
        .ok_or("a candidate the session cannot store was accepted")?;
    match refusal {
        SessionError::AskOptionTooLarge { bytes, limit, .. } => {
            assert_eq!(bytes, option.len(), "the candidate's stored width");
            assert_eq!(limit, 32, "the ceiling the session applies");
        }
        other => {
            return Err(format!("expected the candidate refusal, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn a_template_whose_literals_alone_exceed_the_ceiling_is_refused_at_load() -> TestResult {
    // The one size claim that is true whatever the variables turn out to hold:
    // every render contains the literal bytes, so a template whose literals are
    // over the ceiling can never render. It is refused where it is authored
    // rather than at the node that would speak it.
    let flow = speak_back_flow(&"L".repeat(100), &[String::from("x")])?;
    // Under the shipped ceiling the document is fine — the flow is not
    // malformed, it just cannot be spoken under this session's bound.
    let strict = ResourceLimits::shipped().tighten(ResourceAxis::Record, 64);
    let refusal = Session::with_limits("strict", flow.clone(), strict)
        .err()
        .ok_or("a template whose literals are over the ceiling was accepted")?;
    match refusal {
        SessionError::TemplateExpansionTooLarge { node, bytes, limit } => {
            assert_eq!(node, "say", "the refusal names the template's node");
            assert_eq!(
                bytes, 100,
                "the literal floor, which no render can go below"
            );
            assert_eq!(limit, 64, "the ceiling the session applies");
        }
        other => {
            return Err(format!("expected the literal refusal, got {other:?}").into());
        }
    }
    Session::with_limits("shipped", flow, ResourceLimits::shipped())?;
    Ok(())
}

#[test]
fn ask_nodes_with_no_options_or_a_repeated_option_are_refused_as_malformed() -> TestResult {
    // Each half alone: an empty list, and a list whose two options are the same
    // string. Either one is an ask the person cannot answer.
    let repeated = vec![String::from("a"), String::from("a")];
    for options in [Vec::new(), repeated] {
        match single_ask_flow(VarType::String, options.clone()) {
            Err(SessionError::MalformedFlow { cause }) => {
                assert!(
                    cause.contains("has invalid options"),
                    "the refusal names the options: {cause}"
                );
            }
            other => {
                return Err(
                    format!("expected a malformed ask for {options:?}, got {other:?}").into(),
                );
            }
        }
    }
    Ok(())
}

#[test]
fn an_answer_exactly_at_the_utterance_ceiling_is_heard_and_one_byte_over_is_refused() -> TestResult
{
    // The answer path's ingress boundary: eight bytes under an eight-byte
    // ceiling is heard, nine is refused before a step is charged.
    let ceiling = 8usize;
    let limits = ResourceLimits::shipped().tighten(ResourceAxis::Utterance, ceiling);
    let option = String::from("12345678");
    let mut session = Session::with_limits(
        "at",
        speak_back_flow("${answer}", std::slice::from_ref(&option))?,
        limits,
    )?;
    session.answer(&option)?;
    assert_eq!(
        session.terminal(),
        Some(&Terminal::Completed),
        "an answer of exactly the ceiling is resolved and the flow completes"
    );
    let mut session = Session::with_limits(
        "over",
        speak_back_flow("${answer}", std::slice::from_ref(&option))?,
        limits,
    )?;
    let steps_before = session.steps();
    match session.answer("123456789") {
        Err(SessionError::UtteranceTooLarge { bytes, limit }) => {
            assert_eq!(bytes, 9, "the refusal counts the bytes offered");
            assert_eq!(limit, ceiling, "the refusal reports the ceiling in force");
        }
        other => return Err(format!("expected the utterance refusal, got {other:?}").into()),
    }
    assert_eq!(
        session.steps(),
        steps_before,
        "a refused answer charges no step"
    );
    Ok(())
}

#[test]
fn an_ask_prompt_exactly_at_the_record_ceiling_is_recorded_and_one_byte_over_is_refused()
-> TestResult {
    // The ask prompt's boundary, from both sides. "Choose one: x" is thirteen
    // bytes, so a thirteen-byte record ceiling holds it exactly and twelve is one
    // byte short. A `>=` in the prompt check refuses the exact fit; this pins it.
    let option = String::from("x");
    let prompt = "Choose one: x";
    let ceiling = prompt.len();
    let fits = ResourceLimits::shipped().tighten(ResourceAxis::Record, ceiling);
    let session = Session::with_limits(
        "fits",
        single_ask_flow(VarType::String, vec![option.clone()])?,
        fits,
    )?;
    let asked = session
        .transcript()
        .iter()
        .find(|entry| entry.path_node() == "ask")
        .ok_or("the ask node recorded its prompt")?;
    assert_eq!(asked.text(), prompt, "the prompt fills the ceiling exactly");

    let short = ceiling
        .checked_sub(1)
        .ok_or("the fixture's ceiling underflows")?;
    let tight = ResourceLimits::shipped().tighten(ResourceAxis::Record, short);
    match Session::with_limits(
        "short",
        single_ask_flow(VarType::String, vec![option])?,
        tight,
    ) {
        Err(SessionError::RecordTooLarge { node, bytes, limit }) => {
            assert_eq!(node, "ask", "the refusal names the ask node");
            assert_eq!(bytes, ceiling, "the prompt's computed width");
            assert_eq!(limit, short, "the ceiling the session applies");
        }
        other => {
            return Err(format!("expected the record refusal, got {other:?}").into());
        }
    }
    Ok(())
}
