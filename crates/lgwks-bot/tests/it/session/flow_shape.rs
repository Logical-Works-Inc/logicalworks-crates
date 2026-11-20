//! What a flow document may contain: every node kind, the terminals a session
//! reaches, and the load-time refusals of a malformed shape.

use lgwks_bot::session::{FlowEdge, Predicate, Session, Value, ValueExpr};

use super::*;

/// A `Branch` node on `var`: to `then` when it holds the choice `value`, to
/// `otherwise` when it does not.
fn branch_on(var: &str, value: &str, then: &str, otherwise: &str) -> NodeKind {
    NodeKind::Branch {
        var: String::from(var),
        when: Predicate::Eq(
            ValueExpr::Var(String::from(var)),
            ValueExpr::Literal(Value::Choice(String::from(value))),
        ),
        then: String::from(then),
        otherwise: String::from(otherwise),
    }
}

/// A `Route` node dispatching to `dispatch`, falling back to `fallback`.
fn route(dispatch: &str, fallback: &str) -> NodeKind {
    NodeKind::Route {
        dispatch: String::from(dispatch),
        fallback: String::from(fallback),
    }
}

/// Build a 20-node graph containing every shipped node kind.
fn twenty_node_flow() -> Result<FlowSpec, SessionError> {
    let mut vars = BTreeMap::new();
    vars.insert(String::from("choice"), choice(&["yes", "no"]));
    vars.insert(String::from("decision"), choice(&["go", "stop"]));
    vars.insert(String::from("next"), choice(&["yes", "no"]));

    let mut nodes = BTreeMap::new();
    nodes.insert(String::from("n00"), say("Welcome"));
    nodes.insert(
        String::from("n01"),
        ask("choice", &["yes", "no"], &[("yes", "n02"), ("no", "n05")]),
    );
    nodes.insert(
        String::from("n02"),
        branch_on("choice", "yes", "n03", "n04"),
    );
    nodes.insert(String::from("n03"), say("You chose ${choice}"));
    nodes.insert(
        String::from("n04"),
        refer("docs", "Please read the ${choice} guide"),
    );
    nodes.insert(String::from("n05"), route("n07", "n08"));
    nodes.insert(String::from("n06"), handoff("support"));
    nodes.insert(String::from("n07"), say("Continue"));
    nodes.insert(String::from("n08"), NodeKind::End);
    nodes.insert(String::from("n09"), say("Choose a direction"));
    nodes.insert(
        String::from("n10"),
        ask(
            "decision",
            &["go", "stop"],
            &[("go", "n11"), ("stop", "n12")],
        ),
    );
    nodes.insert(
        String::from("n11"),
        branch_on("decision", "go", "n13", "n14"),
    );
    nodes.insert(String::from("n12"), refer("help", "Stopping here"));
    nodes.insert(String::from("n13"), route("n15", "n16"));
    nodes.insert(String::from("n14"), handoff("specialist"));
    nodes.insert(String::from("n15"), say("One last answer"));
    nodes.insert(String::from("n16"), NodeKind::End);
    nodes.insert(
        String::from("n17"),
        ask("next", &["yes", "no"], &[("yes", "n18"), ("no", "n19")]),
    );
    nodes.insert(
        String::from("n18"),
        refer("next-step", "The next step is ready"),
    );
    nodes.insert(String::from("n19"), NodeKind::End);

    let edges = vec![
        FlowEdge::next("n00", "n01"),
        FlowEdge::next("n03", "n06"),
        FlowEdge::next("n07", "n09"),
        FlowEdge::next("n09", "n10"),
        FlowEdge::next("n15", "n17"),
    ];
    let terminals = BTreeMap::from([
        (String::from("n04"), referred_to("docs")),
        (String::from("n06"), handed_off_to("support")),
        (String::from("n08"), Terminal::Completed),
        (String::from("n12"), referred_to("help")),
        (String::from("n14"), handed_off_to("specialist")),
        (String::from("n16"), refused_for("fallback refused")),
        (String::from("n18"), referred_to("next-step")),
        (String::from("n19"), Terminal::Completed),
    ]);
    FlowSpec::new(vars, "n00", nodes, edges, terminals, FlowBounds::new(64))
}

/// Build a two-node ask/end flow for validation tests.
fn ask_end_flow(
    vars: BTreeMap<String, VarType>,
    routes: BTreeMap<String, String>,
) -> Result<FlowSpec, SessionError> {
    FlowSpec::new(
        vars,
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                NodeKind::Ask {
                    var: String::from("answer"),
                    options: vec![String::from("yes"), String::from("no")],
                    routes,
                },
            ),
            (String::from("end"), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(8),
    )
}

#[test]
fn flow_fixture_has_twenty_nodes_and_every_node_kind() -> TestResult {
    let flow = twenty_node_flow()?;
    assert_eq!(flow.nodes().len(), 20);
    let kinds = flow
        .nodes()
        .values()
        .map(|node| match *node {
            NodeKind::Say { .. } => "say",
            NodeKind::Ask { .. } => "ask",
            NodeKind::Branch { .. } => "branch",
            NodeKind::Handoff { .. } => "handoff",
            NodeKind::Refer { .. } => "refer",
            NodeKind::Route { .. } => "route",
            NodeKind::End => "end",
            _ => "unknown",
        })
        .collect::<Vec<_>>();
    for expected in ["say", "ask", "branch", "handoff", "refer", "route", "end"] {
        assert!(kinds.contains(&expected), "fixture is missing {expected}");
    }
    let json = flow.to_json()?;
    let decoded = FlowSpec::from_json(&json)?;
    assert_eq!(decoded.nodes().len(), 20);
    Ok(())
}

#[test]
fn scripted_sessions_reach_each_terminal_kind() -> TestResult {
    let mut referred = Session::new("referred", terminal_flow(refer("docs", "read this"), None)?)?;
    assert_eq!(referred.terminal(), Some(&referred_to("docs")));
    let answer_after_terminal = referred.answer("unused");
    assert!(matches!(
        answer_after_terminal,
        Err(SessionError::SessionTerminated)
    ));

    let handed_off = Session::new("handoff", terminal_flow(handoff("support"), None)?)?;
    assert_eq!(handed_off.terminal(), Some(&handed_off_to("support")));

    let completed = Session::new("completed", terminal_flow(NodeKind::End, None)?)?;
    assert_eq!(completed.terminal(), Some(&Terminal::Completed));

    let refused = Session::new(
        "refused",
        terminal_flow(NodeKind::End, Some(refused_for("not eligible")))?,
    )?;
    assert_eq!(refused.terminal(), Some(&refused_for("not eligible")));
    Ok(())
}

#[test]
fn reask_keeps_the_cursor_until_a_resolver_match() -> TestResult {
    let flow = ask_end_flow(
        BTreeMap::from([(String::from("answer"), choice(&["yes", "no"]))]),
        BTreeMap::from([
            (String::from("yes"), String::from("end")),
            (String::from("no"), String::from("end")),
        ]),
    )?;
    let mut session = Session::new("reask", flow)?;
    assert_eq!(session.current(), Some("ask"));
    session.answer("maybe")?;
    assert_eq!(session.current(), Some("ask"));
    assert_eq!(session.terminal(), None);
    assert_eq!(session.transcript().len(), 3);
    session.answer("yes")?;
    assert_eq!(session.terminal(), Some(&Terminal::Completed));
    assert_eq!(
        session.scope().get("answer"),
        Some(&Value::Choice(String::from("yes")))
    );
    Ok(())
}

#[test]
fn cycle_over_budget_returns_without_hanging() -> TestResult {
    let flow = FlowSpec::new(
        BTreeMap::from([(String::from("answer"), choice(&["yes"]))]),
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                NodeKind::Ask {
                    var: String::from("answer"),
                    options: vec![String::from("yes")],
                    routes: BTreeMap::from([(String::from("yes"), String::from("loop"))]),
                },
            ),
            (String::from("loop"), route("loop", "loop")),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(4),
    )?;
    let mut session = Session::new("budget", flow)?;
    let error = session.answer("yes");
    assert!(matches!(
        error,
        Err(SessionError::SessionBudgetExceeded {
            steps: 5,
            budget: 4
        })
    ));
    Ok(())
}

#[test]
fn validation_rejects_an_undeclared_transition_target() {
    let result = FlowSpec::new(
        BTreeMap::new(),
        "start",
        BTreeMap::from([(String::from("start"), NodeKind::End)]),
        vec![FlowEdge::next("start", "missing")],
        BTreeMap::new(),
        FlowBounds::new(4),
    );
    assert!(matches!(
        result,
        Err(SessionError::InvalidTransitionTarget { .. })
    ));
}

#[test]
fn validation_rejects_an_ask_option_without_a_route() {
    let result = ask_end_flow(
        BTreeMap::from([(String::from("answer"), choice(&["yes", "no"]))]),
        BTreeMap::from([(String::from("yes"), String::from("end"))]),
    );
    assert!(matches!(result, Err(SessionError::MissingAskRoute { .. })));
}

#[test]
fn validation_rejects_a_declared_variable_without_a_writer() {
    let result = FlowSpec::new(
        BTreeMap::from([(String::from("answer"), choice(&["yes"]))]),
        "end",
        BTreeMap::from([(String::from("end"), NodeKind::End)]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(4),
    );
    assert!(matches!(
        result,
        Err(SessionError::VariableNeverWritten { .. })
    ));
}

#[test]
fn validation_rejects_an_undeclared_template_variable() {
    let result = FlowSpec::new(
        BTreeMap::new(),
        "say",
        BTreeMap::from([
            (String::from("say"), say("Hello ${missing}")),
            (String::from("end"), NodeKind::End),
        ]),
        vec![FlowEdge::next("say", "end")],
        BTreeMap::new(),
        FlowBounds::new(4),
    );
    assert!(matches!(
        result,
        Err(SessionError::UndeclaredVariable { .. })
    ));
}

/// A flow of two `End` nodes, `start` and `second`, with no edge between them
/// and room for `bound` nodes.
fn two_end_flow(second: &str, bound: usize) -> Result<FlowSpec, SessionError> {
    FlowSpec::new(
        BTreeMap::new(),
        "start",
        BTreeMap::from([
            (String::from("start"), NodeKind::End),
            (String::from(second), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(bound),
    )
}

#[test]
fn validation_rejects_an_unreachable_node() {
    let result = two_end_flow("orphan", 4);
    assert!(matches!(result, Err(SessionError::UnreachableNode { .. })));
}

#[test]
fn validation_rejects_nodes_over_the_declared_budget() {
    let result = two_end_flow("second", 1);
    assert!(matches!(
        result,
        Err(SessionError::FlowBudgetExceeded { .. })
    ));
}

#[test]
fn validation_rejects_an_unknown_node_kind() {
    let source = r#"{
        "vars": {},
        "entry": "start",
        "nodes": {"start": "teleport"},
        "edges": [],
        "terminals": {},
        "bounds": {"budget": 4}
    }"#;
    let result = FlowSpec::from_json(source);
    assert!(matches!(result, Err(SessionError::UnknownNodeKind { .. })));
}
