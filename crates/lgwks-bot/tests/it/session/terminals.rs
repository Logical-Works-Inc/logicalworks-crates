//! Declared outcomes against what a node carries: a declaration that repeats
//! its node is executed, and one that contradicts it is refused where it was
//! authored.

use lgwks_bot::session::{Disposition, EffectLedger, Session};

use super::*;

/// One flow per terminal node kind, paired with the outcome that node kind
/// carries before the terminal map is consulted.
///
/// The targets are deliberately ordinary strings: the cross product below uses
/// `elsewhere`, so a test that stopped comparing targets would accept a
/// declaration that hands the person to a different place than the node names.
fn terminal_kinds() -> Vec<(&'static str, NodeKind, Terminal)> {
    vec![
        ("end", NodeKind::End, Terminal::Completed),
        (
            "handoff",
            handoff("external-support"),
            handed_off_to("external-support"),
        ),
        (
            "refer",
            refer("docs", "Read the guide"),
            referred_to("docs"),
        ),
    ]
}

/// Every declared outcome the cross product tries against every terminal node
/// kind.
///
/// One per `Terminal` variant, each with a payload no node above carries, so
/// the comparison that decides accept-or-refuse is doing work on all four: a
/// rule that only compared variant names would accept `HandedOff` against a
/// `refer` node, and a rule that ignored targets would accept a handoff to the
/// wrong place.
fn declared_outcomes() -> Vec<Terminal> {
    vec![
        Terminal::Completed,
        referred_to("elsewhere"),
        handed_off_to("elsewhere"),
        refused_for("not authorized"),
    ]
}

/// The three facts a conflicting-terminal refusal must carry, or a description
/// of why the document was not refused that way.
///
/// Generic over the success type because the same refusal is checked at both
/// doors: `FlowSpec`'s constructors, and `Session::with_components`, which
/// re-validates a flow a caller may have deserialized without going through
/// `FlowSpec::from_json`.
fn conflict_refusal<T>(
    result: Result<T, SessionError>,
) -> Result<(String, Terminal, Terminal), String> {
    match result {
        Err(SessionError::ConflictingTerminalDeclaration {
            node,
            intrinsic,
            declared,
        }) => Ok((node, intrinsic, declared)),
        Err(other) => Err(format!("refused for the wrong reason: {other}")),
        Ok(_) => Err(String::from(
            "accepted a document whose terminal declaration contradicts its node",
        )),
    }
}

/// The disposition a downstream consumer reads from one terminal outcome.
///
/// Routed through the public classifier rather than matched here, because the
/// defect's consequence was downstream: a consumer switches on this, so this is
/// what the assertion has to read.
fn dispatch_of(terminal: &Terminal) -> Disposition {
    terminal.outcome(EffectLedger::new(0, 0)).disposition()
}

#[test]
fn the_handoff_refusal_from_the_report_is_refused_by_from_json() -> TestResult {
    // The document is the one in the defect report. It used to validate, build
    // a session, and complete with `HandedOff { target: "external-support" }`
    // — the authored refusal was accepted, round-tripped through the document,
    // and then discarded at the moment the node ran.
    let document = r#"
    {
      "vars": {},
      "entry": "handoff",
      "nodes": {
        "handoff": {"handoff": {"target": "external-support"}}
      },
      "terminals": {
        "handoff": {"refused": {"reason": "handoff is not authorized"}}
      }
    }
    "#;

    let (node, intrinsic, declared) = conflict_refusal(FlowSpec::from_json(document))?;
    assert_eq!(
        node, "handoff",
        "the refusal names the node carrying both outcomes"
    );
    assert_eq!(
        intrinsic,
        handed_off_to("external-support"),
        "the refusal names the outcome the node's kind carries"
    );
    assert_eq!(
        declared,
        refused_for("handoff is not authorized"),
        "the refusal names the outcome the document declared"
    );
    Ok(())
}

#[test]
fn a_json_terminal_declaration_is_executed_and_not_merely_accepted() -> TestResult {
    // The other half of the rule: a declaration that agrees with its node is
    // accepted, and the session then executes it. The existing acceptance
    // fixtures already put matching duplicates on `handoff` and `refer` nodes;
    // they could not tell the difference between a declaration that is read
    // and one that is ignored, which is the blind spot this pins.
    let document = r#"
    {
      "vars": {},
      "entry": "handoff",
      "nodes": {
        "handoff": {"handoff": {"target": "external-support"}}
      },
      "terminals": {
        "handoff": {"handed_off": {"target": "external-support"}}
      }
    }
    "#;

    let spec = FlowSpec::from_json(document)?;
    let declared = handed_off_to("external-support");
    assert_eq!(
        spec.effective_terminal("handoff"),
        Some(declared.clone()),
        "the validated document exposes the outcome it declares"
    );
    let session = Session::new("json", spec)?;
    assert_eq!(
        session.terminal(),
        Some(&declared),
        "the session executes the outcome the validated document exposes"
    );
    Ok(())
}

#[test]
fn every_terminal_node_kind_agrees_with_every_declared_outcome() -> TestResult {
    // The cross product: three terminal node kinds against the four `Terminal`
    // variants, plus each node's own outcome, which is the declaration a
    // `handoff` or `refer` node has to be able to repeat. Fourteen
    // combinations, and every one of them is decided here rather than left to
    // the happy path:
    //
    //   - an `end` node accepts all four declared outcomes, because it has no
    //     destination to contradict;
    //   - a `handoff` or `refer` node accepts only the outcome it carries
    //     itself, and refuses the other four.
    //
    // Six accepted and eight refused. The rule is restated here rather than
    // read from the crate because this is the test that says the rule is what a
    // reader would expect: the assertions below are still about behaviour —
    // what the accepted document exposes and executes, and what the refusal
    // names.
    let mut accepted = 0_usize;
    let mut refused = 0_usize;

    for (label, kind, intrinsic) in terminal_kinds() {
        let mut attempts = declared_outcomes();
        if !attempts.contains(&intrinsic) {
            attempts.push(intrinsic.clone());
        }
        for declared in attempts {
            let result = terminal_flow(kind.clone(), Some(declared.clone()));
            if matches!(kind, NodeKind::End) || intrinsic == declared {
                let spec = result.map_err(|error| {
                    format!("{label} refused the outcome {declared:?} it should accept: {error}")
                })?;
                assert_eq!(
                    spec.effective_terminal("start"),
                    Some(declared.clone()),
                    "{label} exposes the outcome it declares"
                );
                let session = Session::new("cross", spec)?;
                assert_eq!(
                    session.current(),
                    None,
                    "{label} stops the cursor at the terminal node"
                );
                assert_eq!(
                    session.terminal(),
                    Some(&declared),
                    "{label} executes the outcome it declares"
                );
                match session.terminal() {
                    Some(terminal) => assert_eq!(
                        dispatch_of(terminal),
                        dispatch_of(&declared),
                        "{label} dispatches as the outcome it declares"
                    ),
                    None => return Err(format!("{label} reached no terminal outcome").into()),
                }
                accepted = accepted.saturating_add(1);
            } else {
                let (node, got_intrinsic, got_declared) = conflict_refusal(result)?;
                assert_eq!(
                    node, "start",
                    "{label} names the node carrying both outcomes"
                );
                assert_eq!(
                    got_intrinsic, intrinsic,
                    "{label} reports the outcome its kind carries"
                );
                assert_eq!(
                    got_declared, declared,
                    "{label} reports the outcome the document declared"
                );
                refused = refused.saturating_add(1);
            }
        }
    }

    assert_eq!(
        (accepted, refused),
        (6, 8),
        "the cross product is complete: every combination is decided"
    );
    Ok(())
}

#[test]
fn a_conflicting_terminal_is_refused_before_the_session_writes_anything() -> TestResult {
    // `json::from_str` is the public door that skips `FlowSpec::from_json`'s
    // validation, so this is a document a caller can genuinely hold:
    // deserialized and never validated. It is therefore the document `Session`
    // has to refuse for itself, and the assertion is that it does so before a
    // single record reaches the journal or the transcript.
    let document = r#"
    {
      "vars": {},
      "entry": "handoff",
      "nodes": {"handoff": {"handoff": {"target": "external-support"}}},
      "terminals": {
        "handoff": {"refused": {"reason": "handoff is not authorized"}}
      }
    }
    "#;

    let (journal, records) = SharedJournal::new();
    let unvalidated: FlowSpec = lgwks_bot::json::from_str(document)?;
    let outcome = Session::with_components(
        "conflict",
        unvalidated,
        lgwks_bot::language::LanguageResolver::new(),
        journal,
    );
    let (node, intrinsic, declared) = conflict_refusal(outcome)?;
    assert_eq!(
        node, "handoff",
        "the refusal names the node carrying both outcomes"
    );
    assert_eq!(
        intrinsic,
        handed_off_to("external-support"),
        "the refusal names the outcome the node's kind carries"
    );
    assert_eq!(
        declared,
        refused_for("handoff is not authorized"),
        "the refusal names the outcome the document declared"
    );
    assert!(
        records.borrow().is_empty(),
        "a refused document reaches the journal with no records: {:?}",
        records.borrow()
    );

    // The same document through the ordinary constructor, since that is how a
    // caller who never touched the facade would arrive at it.
    let unvalidated: FlowSpec = lgwks_bot::json::from_str(document)?;
    assert!(
        matches!(
            Session::new("conflict", unvalidated),
            Err(SessionError::ConflictingTerminalDeclaration { .. })
        ),
        "Session::new refuses the same contradiction"
    );
    Ok(())
}

#[test]
fn a_refused_declaration_never_dispatches_as_a_handoff() -> TestResult {
    // The downstream consequence the report names. A consumer switches on the
    // returned disposition, and the bug handed it a permissive handoff for a
    // document that had explicitly refused. `End` is where a refusal is
    // supported, so that is where the dispatch assertion belongs.
    let spec = terminal_flow(NodeKind::End, Some(refused_for("not authorized")))?;
    let session = Session::new("refused", spec)?;
    let Some(terminal) = session.terminal() else {
        return Err("an end-only flow reached no terminal outcome".into());
    };
    assert_eq!(
        terminal,
        &refused_for("not authorized"),
        "the refusal is what the document declared"
    );
    assert_eq!(
        dispatch_of(terminal),
        Disposition::Refused,
        "a refused document dispatches as refused"
    );
    assert_ne!(
        dispatch_of(terminal),
        Disposition::HandedOff,
        "a refused document is never classified as a handoff"
    );

    // And the same refusal declared on a handoff node is refused outright,
    // rather than run with the refusal discarded — which is the only other way
    // this defect could have been "fixed".
    let conflicting = terminal_flow(
        handoff("external-support"),
        Some(refused_for("not authorized")),
    );
    conflict_refusal(conflicting)?;
    Ok(())
}

#[test]
fn a_non_terminal_node_has_no_effective_outcome() -> TestResult {
    // The `None` half of the centralised calculation, which is what keeps it
    // from inventing an outcome for a node that never ends a run.
    let flow = one_question_flow()?;
    assert_eq!(
        flow.effective_terminal("ask"),
        None,
        "an ask node has no effective outcome"
    );
    assert_eq!(
        flow.effective_terminal("done"),
        Some(Terminal::Completed),
        "an end node with no declaration completes"
    );
    assert_eq!(
        flow.effective_terminal("no-such-node"),
        None,
        "a missing node has no effective outcome"
    );
    Ok(())
}
