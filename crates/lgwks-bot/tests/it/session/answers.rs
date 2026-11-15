//! What an option stores and routes to: the declared type decides, so a signed
//! number is a value, a label is a label, and an option the variable cannot
//! hold is refused when the document loads.

use lgwks_bot::{AnswerRejection, MatchTier, Session, TranscriptEntry, Value};

use super::*;

/// An ask/route flow whose every option routes to a `Refer` terminal named
/// after the option itself.
///
/// The option text is the route identity on purpose: "the session went where
/// the value says" then reads as one equality against the option, and a test
/// never has to know a node id to assert a route.
fn option_routed_flow(
    var: &str,
    declared: VarType,
    options: &[&str],
) -> Result<FlowSpec, SessionError> {
    let offered: Vec<String> = options.iter().map(|option| (*option).to_owned()).collect();
    let mut routes = BTreeMap::new();
    let mut nodes = BTreeMap::new();
    let mut terminals = BTreeMap::new();
    for (position, option) in offered.iter().enumerate() {
        let target = format!("routed_{position}");
        routes.insert(option.clone(), target.clone());
        nodes.insert(
            target.clone(),
            NodeKind::Refer {
                target: option.clone(),
                text: String::from("Noted"),
            },
        );
        terminals.insert(
            target,
            Terminal::Referred {
                target: option.clone(),
            },
        );
    }
    nodes.insert(
        String::from("ask"),
        NodeKind::Ask {
            var: String::from(var),
            options: offered,
            routes,
        },
    );
    FlowSpec::new(
        BTreeMap::from([(String::from(var), declared)]),
        "ask",
        nodes,
        Vec::new(),
        terminals,
        FlowBounds::new(16),
    )
}

/// Reads the route a session took as the option text it named.
fn routed_to(session: &Session) -> Option<String> {
    session.terminal().and_then(|terminal| match *terminal {
        Terminal::Referred { ref target } => Some(target.clone()),
        _ => None,
    })
}

/// The declared-type label of one stored value.
///
/// Written out here rather than read from [`VarType::label`] so the assertion
/// is about the value that was actually stored: comparing a stored value to the
/// declaration through one function would agree with itself whatever it stored.
fn value_kind(value: &Value) -> &'static str {
    match *value {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Boolean(_) => "boolean",
        Value::Choice(_) => "choice",
        // `Value` is `#[non_exhaustive]`. A variant added later must be named
        // above before this can claim to know what the variable stored; until
        // then it reports a string no declared type matches, so the assertion
        // that reads it fails rather than passing by accident.
        _ => "unrecognised",
    }
}

/// The five facts an ask-candidate refusal must carry, or a description of why
/// the flow was not refused that way.
fn ask_refusal(result: Result<FlowSpec, SessionError>) -> Result<[String; 5], String> {
    match result {
        Err(SessionError::AskOptionNotAssignable {
            node,
            variable,
            option,
            expected,
            cause,
        }) => Ok([node, variable, option, String::from(expected), cause]),
        Err(other) => Err(format!("refused for the wrong reason: {other}")),
        Ok(spec) => Err(format!(
            "accepted a flow that offers options it cannot store ({} nodes)",
            spec.nodes().len()
        )),
    }
}

/// The filed counterexample for #38, at session level: `-5` is not one of the
/// two numbers this question offers.
///
/// Before the fix, normalization folded the sign away, `-5` matched `"5"` at
/// the Exact tier, and the session stored `Value::Integer(5)` and advanced down
/// the positive route — an answer the person never gave, reported at maximum
/// confidence.
#[test]
fn a_negative_answer_is_not_folded_into_the_positive_option() -> TestResult {
    let mut session = Session::new(
        "sign",
        option_routed_flow("amount", VarType::Integer, &["5", "10"])?,
    )?;

    session.answer("-5")?;

    assert_eq!(
        session.scope().get("amount"),
        None,
        "a number the question does not offer must not be stored as one it does"
    );
    assert_eq!(
        session.current(),
        Some("ask"),
        "the question is re-asked rather than answered by a different number"
    );
    assert_eq!(
        session.terminal(),
        None,
        "and no route is taken, so nothing downstream acts on the invented value"
    );
    Ok(())
}

/// The value, not the spelling, decides: every reading of one number lands on
/// that number's option, and both signed choices stay distinguishable.
#[test]
fn an_integer_answer_stores_its_value_and_takes_its_own_route() -> TestResult {
    for (utterance, expected_value, expected_option) in [
        ("-5", -5_i64, "-5"),
        ("5", 5, "5"),
        ("+5", 5, "5"),
        (" 5 ", 5, "5"),
        ("0", 0, "0"),
        ("10", 10, "10"),
    ] {
        let mut session = Session::new(
            "values",
            option_routed_flow("amount", VarType::Integer, &["-5", "5", "0", "10"])?,
        )?;
        session.answer(utterance)?;
        assert_eq!(
            session.scope().get("amount"),
            Some(&Value::Integer(expected_value)),
            "{utterance:?} must store the parsed value, not the option's spelling"
        );
        assert_eq!(
            routed_to(&session).as_deref(),
            Some(expected_option),
            "{utterance:?} must take the route of the option holding that value"
        );
    }
    Ok(())
}

/// The ends of the range are values like any other, and one past them is not a
/// value at all.
#[test]
fn the_integer_boundaries_round_trip_and_overflow_is_refused() -> TestResult {
    let low = i64::MIN.to_string();
    let high = i64::MAX.to_string();
    for (utterance, expected) in [(low.as_str(), i64::MIN), (high.as_str(), i64::MAX)] {
        let mut session = Session::new(
            "bounds",
            option_routed_flow("amount", VarType::Integer, &[low.as_str(), high.as_str()])?,
        )?;
        session.answer(utterance)?;
        assert_eq!(
            session.scope().get("amount"),
            Some(&Value::Integer(expected)),
            "{utterance:?} is representable and must be stored exactly"
        );
        assert_eq!(
            routed_to(&session).as_deref(),
            Some(utterance),
            "{utterance:?} must take the route of the option holding that value"
        );
    }

    // One past each end parses as nothing, so it matches nothing: the failure
    // is a re-ask, never a wrapped or clamped value.
    for utterance in ["9223372036854775808", "-9223372036854775809"] {
        let mut session = Session::new(
            "overflow",
            option_routed_flow("amount", VarType::Integer, &["5", "10"])?,
        )?;
        session.answer(utterance)?;
        assert_eq!(
            session.scope().get("amount"),
            None,
            "{utterance:?} is out of range and must not be stored"
        );
        assert_eq!(
            session.current(),
            Some("ask"),
            "{utterance:?} must re-ask rather than wrap into a nearby value"
        );
    }
    Ok(())
}

/// The control for #38: folding punctuation away is still what a *label*
/// question wants, and the fix must not have withdrawn it.
#[test]
fn punctuation_tolerant_matching_still_answers_a_label_question() -> TestResult {
    let mut session = Session::new(
        "labels",
        option_routed_flow("answer", VarType::Boolean, &["yes", "no"])?,
    )?;

    session.answer("yes!")?;

    assert_eq!(
        session.scope().get("answer"),
        Some(&Value::Boolean(true)),
        "an ordinary word answer keeps the tolerant reading it has always had"
    );
    assert_eq!(
        routed_to(&session).as_deref(),
        Some("yes"),
        "and still takes the route of the option it matched"
    );
    Ok(())
}

/// The policy is what decides, so the same authored strings are one answer or
/// two depending on the declared type: `-5` and `5` are two values and one
/// label, and `5` and `05` are two spellings of one value.
#[test]
fn option_collisions_are_judged_under_the_declared_policy() -> TestResult {
    let as_values = option_routed_flow("amount", VarType::Integer, &["-5", "5"])?;
    assert_eq!(
        as_values.nodes().len(),
        3,
        "a signed question offering both signs is accepted: two values"
    );

    let as_labels = option_routed_flow("answer", VarType::String, &["-5", "5"]);
    assert!(
        matches!(as_labels, Err(SessionError::MalformedFlow { .. })),
        "the same two strings are one label once the sign folds, and a flow that \
         offers them that way can never tell the person's answer apart: {as_labels:?}"
    );

    let duplicate_values = option_routed_flow("amount", VarType::Integer, &["5", "05"]);
    assert!(
        matches!(duplicate_values, Err(SessionError::MalformedFlow { .. })),
        "two options holding one value are the numeric form of the same \
         collision: {duplicate_values:?}"
    );
    Ok(())
}

#[test]
fn the_boolean_ask_from_the_report_is_refused_by_from_json() -> TestResult {
    // The document is the one in the defect report, verbatim. It used to
    // validate, construct a session, reach the ask, and then fail *every*
    // answer: `Continue` and `Cancel` are exact resolver matches that a boolean
    // variable cannot store, and answering `true` cannot repair the flow
    // because the session stores the selected option, not the raw utterance.
    let document = r#"
    {
      "vars": {"decision": "boolean"},
      "entry": "ask",
      "nodes": {
        "ask": {"ask": {
          "var": "decision",
          "options": ["Continue", "Cancel"],
          "routes": {"Continue": "done", "Cancel": "done"}
        }},
        "done": "end"
      }
    }
    "#;

    // `from_json` is the whole proof: the refusal is a load-time verdict, so no
    // session is constructed and no journal entry is written.
    let [node, variable, option, expected, cause] = ask_refusal(FlowSpec::from_json(document))?;
    assert_eq!(
        node, "ask",
        "the refusal names the node carrying the candidate"
    );
    assert_eq!(
        variable, "decision",
        "the refusal names the variable the ask writes"
    );
    assert_eq!(
        option, "Continue",
        "the refusal names the candidate it could not store"
    );
    assert_eq!(expected, "boolean", "the refusal names the declared type");
    assert_eq!(
        cause,
        AnswerRejection::NotABoolean.to_string(),
        "the cause is the decoder's own verdict rather than a second opinion about it"
    );
    Ok(())
}

#[test]
fn a_candidate_after_a_storable_one_is_still_refused() -> TestResult {
    // A check that stopped at the first option would accept this document and
    // leave `Cancel` unanswerable: the same defect, one option later.
    let facts = ask_refusal(single_ask_flow(
        VarType::Boolean,
        vec![String::from("yes"), String::from("Cancel")],
    ))?;
    let [_, _, option, _, _] = facts;
    assert_eq!(
        option, "Cancel",
        "every candidate is checked, not only the first"
    );
    Ok(())
}

#[test]
fn every_declared_type_refuses_a_candidate_it_cannot_store() -> TestResult {
    // `String` has no refusal case to pin: it stores any candidate, which is
    // the free-form declaration doing its job. The other three each have a
    // candidate that a person could reasonably author and that the variable
    // cannot hold, plus the two integer range edges.
    let choice = choice(&["yes", "no"]);
    let cases: Vec<(VarType, Vec<String>, &str, AnswerRejection)> = vec![
        (
            VarType::Boolean,
            vec![String::from("Continue"), String::from("Cancel")],
            "Continue",
            AnswerRejection::NotABoolean,
        ),
        (
            VarType::Integer,
            vec![String::from("7.5")],
            "7.5",
            AnswerRejection::NotAnInteger,
        ),
        (
            VarType::Integer,
            vec![String::from("9223372036854775808")],
            "9223372036854775808",
            AnswerRejection::IntegerOutOfRange,
        ),
        (
            VarType::Integer,
            vec![String::from("-9223372036854775809")],
            "-9223372036854775809",
            AnswerRejection::IntegerOutOfRange,
        ),
        (
            choice,
            vec![String::from("approve"), String::from("deny")],
            "approve",
            AnswerRejection::NotADeclaredChoice,
        ),
    ];

    for (declared, options, expected_option, rejection) in cases {
        let expected_label = declared.label();
        let [node, variable, option, expected, cause] =
            ask_refusal(single_ask_flow(declared, options))?;
        assert_eq!(node, "ask", "the refusal names the ask node");
        assert_eq!(
            variable, "answer",
            "the refusal names the variable the ask writes"
        );
        assert_eq!(
            option, expected_option,
            "the refusal names the candidate it cannot store"
        );
        assert_eq!(
            expected, expected_label,
            "the refusal names the declared type"
        );
        assert_eq!(
            cause,
            rejection.to_string(),
            "the cause is the decoder's own verdict"
        );
    }
    Ok(())
}

#[test]
fn every_option_of_every_accepted_ask_is_storable() -> TestResult {
    // Four asks, one per declared type, chained so every node is reachable:
    // each ask routes all of its options to the next ask, and the last one to
    // the end. The candidates disagree with the declaration in spelling on
    // purpose (`SMALL` against `small`, `007` against a plain integer), because
    // those are exactly the candidates a stricter check would refuse and a
    // person would still expect to work.
    //
    // The three integer candidates name three *different* values, and they have
    // to: two options naming one value are refused at load
    // (`option_collisions_are_judged_under_the_declared_policy`), because the
    // resolver can only ever report the tie and the person cannot answer the
    // question. So the zero-padded spelling here is a pad of no other candidate.
    let asks = [
        (
            String::from("words"),
            VarType::String,
            vec![String::from("Continue"), String::from("Cancel")],
        ),
        (
            String::from("count"),
            VarType::Integer,
            vec![
                String::from("8"),
                String::from(" -12 "),
                String::from("007"),
            ],
        ),
        (
            String::from("confirmed"),
            VarType::Boolean,
            vec![
                String::from("yes"),
                String::from("FALSE"),
                String::from("No"),
            ],
        ),
        (
            String::from("size"),
            choice(&["small", "large"]),
            vec![String::from("SMALL"), String::from("Large")],
        ),
    ];

    let ids: Vec<String> = asks.iter().map(|ask| format!("ask_{}", ask.0)).collect();
    let mut vars = BTreeMap::new();
    let mut nodes = BTreeMap::new();
    for (position, ask) in asks.iter().enumerate() {
        let (variable, declared, options) = (&ask.0, &ask.1, &ask.2);
        vars.insert(variable.clone(), declared.clone());
        // The last ask routes to the session's own end node, which has no id in
        // the ask list; every earlier ask routes to the next one.
        let target = match ids.get(position.saturating_add(1)) {
            Some(next) => next.clone(),
            None => String::from("end"),
        };
        let routes = options
            .iter()
            .map(|option| (option.clone(), target.clone()))
            .collect();
        nodes.insert(
            format!("ask_{variable}"),
            NodeKind::Ask {
                var: variable.clone(),
                options: options.clone(),
                routes,
            },
        );
    }
    nodes.insert(String::from("end"), NodeKind::End);
    let spec = FlowSpec::from_nodes(vars, "ask_words", nodes, FlowBounds::new(64))?;

    // The enumeration is over the *spec*, not over the `asks` array, so it
    // proves the property of the document that was accepted rather than of the
    // table the document was built from.
    let mut enumerated = 0_usize;
    for node_id in spec.nodes().keys() {
        let Some(NodeKind::Ask { var, options, .. }) = spec.node(node_id).cloned() else {
            continue;
        };
        let Some(declared) = spec.vars().get(&var) else {
            return Err(format!("ask node {node_id} writes undeclared variable {var}").into());
        };
        for option in &options {
            let decoded = declared.decode_answer(option).map_err(|rejection| {
                format!(
                    "accepted ask {node_id} offers {option:?} for a {} variable, \
                     which cannot store it: {rejection}",
                    declared.label()
                )
            })?;
            assert_eq!(
                value_kind(&decoded),
                declared.label(),
                "ask {node_id} option {option:?} stores a {} value",
                value_kind(&decoded)
            );
            enumerated = enumerated.saturating_add(1);
        }
    }
    assert_eq!(
        enumerated, 10,
        "the fixture's four accepted asks offer ten candidates in total"
    );
    Ok(())
}

#[test]
fn a_display_label_stores_the_value_the_declared_type_names() -> TestResult {
    // A candidate is a label the resolver matches *and* a value the variable
    // stores, and the two are not always the same string. Each case pins one
    // mapping a person reading the document would expect: the label is routed
    // on as written, and what lands in the variable is the declared type's
    // value.
    let cases: Vec<(VarType, &str, Value)> = vec![
        (VarType::Boolean, "yes", Value::Boolean(true)),
        (VarType::Boolean, "No", Value::Boolean(false)),
        (VarType::Integer, "007", Value::Integer(7)),
        (VarType::Integer, " -12 ", Value::Integer(-12)),
        (
            choice(&["small"]),
            "SMALL",
            Value::Choice(String::from("small")),
        ),
        (
            VarType::String,
            "  padded  ",
            Value::String(String::from("  padded  ")),
        ),
    ];

    for (declared, option, expected) in cases {
        let label = declared.label();
        let spec = single_ask_flow(declared, vec![String::from(option)])?;
        let mut session = Session::new("mapping", spec)?;
        session.answer(option)?;
        assert_eq!(
            session.scope().get("answer"),
            Some(&expected),
            "answering {option:?} for a {label} variable stores {expected:?}"
        );
    }
    Ok(())
}

#[test]
fn an_ambiguous_answer_tied_two_ways_narrows_the_reask_to_those_two() -> TestResult {
    // Three options, a tie between the first and the last: the re-ask names
    // exactly those two. Two is the smallest set that is still a question, so a
    // threshold above two would repeat the whole list and the person could loop.
    let options = ["red", "green", "blue"];
    let flow = FlowSpec::new(
        BTreeMap::from([(String::from("colour"), choice(&options))]),
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                ask(
                    "colour",
                    &options,
                    &[("red", "done"), ("green", "done"), ("blue", "done")],
                ),
            ),
            (String::from("done"), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(8),
    )?;
    let mut session = Session::with_resolver(
        "two-way",
        flow,
        FixedResolver(Resolution::Ambiguous {
            tied: vec![0, 2],
            tier: MatchTier::Fuzzy,
            score: 0.8,
        }),
    )?;
    session.answer("reddish blue")?;
    assert_eq!(
        session.current(),
        Some("ask"),
        "a tie leaves the cursor on the ask"
    );
    assert_eq!(
        session.transcript().last().map(TranscriptEntry::text),
        Some("Choose one: red, blue"),
        "the re-ask names only the two tied options"
    );
    Ok(())
}

#[test]
fn exact_ambiguous_and_absent_answers_behave_as_before() -> TestResult {
    // The load-time check must not change what happens to an answer at runtime.
    let mut exact = Session::new("exact", one_question_flow()?)?;
    exact.answer("yes")?;
    assert_eq!(
        exact.terminal(),
        Some(&Terminal::Completed),
        "an exact match still routes on a validated ask"
    );
    assert_eq!(
        exact.scope().get("choice"),
        Some(&Value::Choice(String::from("yes"))),
        "the exact match stores the option the route was keyed by"
    );

    let mut ambiguous = Session::with_resolver(
        "ambiguous",
        one_question_flow()?,
        FixedResolver(Resolution::Ambiguous {
            tied: vec![0, 1],
            tier: MatchTier::Fuzzy,
            score: 0.8,
        }),
    )?;
    ambiguous.answer("maybe")?;
    assert_eq!(
        ambiguous.current(),
        Some("ask"),
        "an ambiguous answer leaves the cursor on the ask"
    );
    assert_eq!(
        ambiguous.scope().get("choice"),
        None,
        "an ambiguous answer stores no value"
    );
    assert_eq!(
        ambiguous.transcript().last().map(TranscriptEntry::text),
        Some("Choose one: yes, no"),
        "the re-ask repeats the options still in play"
    );

    let mut absent = Session::with_resolver(
        "absent",
        one_question_flow()?,
        FixedResolver(Resolution::Absent { best_score: 0.0 }),
    )?;
    absent.answer("maybe")?;
    assert_eq!(
        absent.current(),
        Some("ask"),
        "an unrecognized answer leaves the cursor on the ask"
    );
    assert_eq!(
        absent.scope().get("choice"),
        None,
        "an unrecognized answer stores no value"
    );
    Ok(())
}
