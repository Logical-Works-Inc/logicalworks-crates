//! Definite assignment over the execution graph (issue #36): a variable read
//! before any path assigns it is refused when the document loads.

use std::sync::{Arc, Mutex};

use lgwks_bot::{FlowEdge, MatchTier, Predicate, Session};

use super::*;
use crate::lock::take_unpoisoned;

/// A journal that keeps every record it receives, so a test can assert what
/// the runner emitted. Shared rather than moved so the assertion can read the
/// records back after [`Session`] has taken ownership of its sink.
#[derive(Clone, Default)]
struct RecordingJournal {
    records: Arc<Mutex<Vec<String>>>,
    /// Where the receipts go, for the same reason [`SharedJournal`] delegates
    /// them: this double is about the records, and a receipt it accepted must
    /// actually be held somewhere rather than reported accepted and dropped.
    receipts: MemoryJournal,
}

impl RecordingJournal {
    fn new() -> Self {
        Self::default()
    }

    /// Every record received so far, oldest first.
    ///
    /// A poisoned lock yields the records as they stand, because each one is
    /// pushed by a single statement and a panic between two pushes cannot leave a
    /// half-written entry; the positive control in
    /// `validation_refuses_before_any_journal_record` fails if this read ever
    /// comes back empty, so an empty read cannot pass for the wrong reason.
    fn records(&self) -> Vec<String> {
        take_unpoisoned(&self.records).clone()
    }
}

impl Journal for RecordingJournal {
    fn record(&mut self, path_node: &str, role: &str, text: &str) {
        if let Ok(mut records) = self.records.lock() {
            records.push(format!("{path_node}/{role}/{text}"));
        }
    }

    /// Forwarded whole to [`MemoryJournal`], for the reason its sibling above
    /// gives.
    fn record_decision(
        &mut self,
        receipt: &DecisionReceipt,
    ) -> Result<ReceiptAcceptance, JournalError> {
        self.receipts.record_decision(receipt)
    }
}

/// The literal accepted-but-failing document from issue #36.
///
/// Every node is reachable and `name` has a writer, so the global writer set is
/// satisfied; the only writer runs *after* the read, and the session starts
/// with an empty scope, so `Session::new` failed on the first step with
/// `VariableUnset` before the user could reach the ask.
const ENTRY_READ_BEFORE_WRITE: &str = r#"{
  "vars": {"name": "string"},
  "entry": "greet",
  "nodes": {
    "greet": {"say": {"text": "Hello ${name}"}},
    "ask": {"ask": {
      "var": "name",
      "options": ["Ada"],
      "routes": {"Ada": "end"}
    }},
    "end": "end"
  },
  "edges": [{"next": {"from": "greet", "to": "ask"}}],
  "bounds": {"budget": 8}
}"#;

/// Build a flow from a node map, an entry, and explicit continuations.
fn flow_with(
    vars: BTreeMap<String, VarType>,
    entry: &str,
    nodes: BTreeMap<String, NodeKind>,
    edges: Vec<FlowEdge>,
    terminals: BTreeMap<String, Terminal>,
) -> Result<FlowSpec, SessionError> {
    FlowSpec::new(vars, entry, nodes, edges, terminals, FlowBounds::new(32))
}

/// An ask that routes every option to `target`.
fn ask_routing(var: &str, options: [&str; 2], target: &str) -> NodeKind {
    NodeKind::Ask {
        var: String::from(var),
        options: options.iter().map(|option| String::from(*option)).collect(),
        routes: options
            .iter()
            .map(|option| (String::from(*option), String::from(target)))
            .collect(),
    }
}

/// The one variable the read-before-init flows declare: `answer`, a yes/no
/// choice.
fn yes_no_answer() -> BTreeMap<String, VarType> {
    BTreeMap::from([(String::from("answer"), choice(&["yes", "no"]))])
}

/// The opening the one-armed-writer flows share: a `start` branch on `answer`
/// whose `then` arm asks for it and routes on to `asked`, and whose other arm
/// goes to `otherwise` without asking.
fn asked_on_one_arm(otherwise: &str, asked: &str) -> BTreeMap<String, NodeKind> {
    BTreeMap::from([
        (String::from("start"), split_on_answer("ask", otherwise)),
        (
            String::from("ask"),
            ask_routing("answer", ["yes", "no"], asked),
        ),
    ])
}

/// A `Branch` on `answer` whose predicate can take either arm, to `then` or
/// to `otherwise`, so validation must consider both paths.
fn split_on_answer(then: &str, otherwise: &str) -> NodeKind {
    NodeKind::Branch {
        var: String::from("answer"),
        when: both_arms(),
        then: String::from(then),
        otherwise: String::from(otherwise),
    }
}

/// A predicate that can take both arms, so the analysis is forced to treat
/// both continuations as feasible.
fn both_arms() -> Predicate {
    Predicate::Const(true)
}

#[test]
fn validation_rejects_an_entry_read_before_any_writer() -> TestResult {
    let error = FlowSpec::from_json(ENTRY_READ_BEFORE_WRITE)
        .err()
        .ok_or("a read with no reaching assignment must be refused at load time")?;
    assert!(
        matches!(
            error,
            SessionError::VariableReadBeforeInit { ref node, ref name }
                if node == "greet" && name == "name"
        ),
        "the diagnostic must name the reading node and the variable: {error}"
    );
    Ok(())
}

#[test]
fn validation_rejects_a_branch_path_that_bypasses_initialization() -> TestResult {
    // `answer` is assignable, and every node is reachable — the defect is that
    // the writer sits on one arm of the branch while the read sits after the
    // join, so the other arm reaches the read with an empty scope.
    let declarations = yes_no_answer();
    let mut nodes = asked_on_one_arm("read", "read");
    nodes.extend([
        (String::from("read"), say("You said ${answer}")),
        (String::from("end"), NodeKind::End),
    ]);
    let error = flow_with(
        declarations,
        "start",
        nodes,
        vec![FlowEdge::next("read", "end")],
        BTreeMap::from([(String::from("end"), Terminal::Completed)]),
    )
    .err()
    .ok_or("a read reachable through an uninitialized arm must be refused")?;
    assert!(
        matches!(
            error,
            SessionError::VariableReadBeforeInit { ref node, ref name }
                if node == "read" && name == "answer"
        ),
        "the diagnostic must name the reading node and the variable: {error}"
    );
    Ok(())
}

#[test]
fn validation_rejects_a_read_in_a_cycle_before_its_writer() -> TestResult {
    // The read is the entry and the writer is one edge further on, so the
    // cycle re-reads it every iteration; the fixed point must not let the
    // back edge supply the assignment to the node that precedes it.
    let declarations = yes_no_answer();
    let nodes = BTreeMap::from([
        (String::from("head"), say("You said ${answer}")),
        (
            String::from("ask"),
            ask_routing("answer", ["yes", "no"], "head"),
        ),
    ]);
    let error = flow_with(
        declarations,
        "head",
        nodes,
        vec![FlowEdge::next("head", "ask")],
        BTreeMap::new(),
    )
    .err()
    .ok_or("a read that only a back edge's writer reaches must be refused")?;
    assert!(
        matches!(
            error,
            SessionError::VariableReadBeforeInit { ref node, ref name }
                if node == "head" && name == "answer"
        ),
        "the diagnostic must name the reading node and the variable: {error}"
    );
    Ok(())
}

#[test]
fn validation_accepts_an_ask_read_by_its_successor() -> TestResult {
    let declarations = yes_no_answer();
    let nodes = BTreeMap::from([
        (
            String::from("ask"),
            ask_routing("answer", ["yes", "no"], "read"),
        ),
        (String::from("read"), say("You said ${answer}")),
        (String::from("end"), NodeKind::End),
    ]);
    let flow = flow_with(
        declarations,
        "ask",
        nodes,
        vec![FlowEdge::next("read", "end")],
        BTreeMap::from([(String::from("end"), Terminal::Completed)]),
    )?;
    // The verdict is only half the claim: a session over the accepted flow
    // must actually run. It stops at the ask awaiting an answer, which is a
    // node it could only reach by executing the entry — not by failing on an
    // unassigned read.
    let session = Session::new("review", flow)?;
    assert_eq!(
        session.current(),
        Some("ask"),
        "an accepted ask-then-read flow runs to its ask"
    );
    Ok(())
}

#[test]
fn validation_accepts_a_join_where_every_predecessor_assigns() -> TestResult {
    let declarations = yes_no_answer();
    let nodes = BTreeMap::from([
        (String::from("start"), split_on_answer("ask_a", "ask_b")),
        (
            String::from("ask_a"),
            ask_routing("answer", ["yes", "no"], "join"),
        ),
        (
            String::from("ask_b"),
            ask_routing("answer", ["yes", "no"], "join"),
        ),
        (String::from("join"), say("You said ${answer}")),
        (String::from("end"), NodeKind::End),
    ]);
    flow_with(
        declarations,
        "start",
        nodes,
        vec![FlowEdge::next("join", "end")],
        BTreeMap::from([(String::from("end"), Terminal::Completed)]),
    )?;
    Ok(())
}

#[test]
fn validation_rejects_a_join_where_one_predecessor_assigns() -> TestResult {
    let declarations = yes_no_answer();
    let mut nodes = asked_on_one_arm("skip", "join");
    nodes.extend([
        (String::from("skip"), say("No answer taken")),
        (String::from("join"), say("You said ${answer}")),
        (String::from("end"), NodeKind::End),
    ]);
    let error = flow_with(
        declarations,
        "start",
        nodes,
        vec![
            FlowEdge::next("skip", "join"),
            FlowEdge::next("join", "end"),
        ],
        BTreeMap::from([(String::from("end"), Terminal::Completed)]),
    )
    .err()
    .ok_or("a join that one predecessor reaches unassigned must be refused")?;
    assert!(
        matches!(
            error,
            SessionError::VariableReadBeforeInit { ref node, ref name }
                if node == "join" && name == "answer"
        ),
        "the diagnostic must name the reading node and the variable: {error}"
    );
    Ok(())
}

/// A resolver that never recognizes an answer, so the flows below stop at
/// their ask instead of driving past it.
const UNRESOLVING: Resolution = Resolution::Absent { best_score: 0.0 };

#[test]
fn validation_refuses_before_any_journal_record() -> TestResult {
    // The ordering claim: validation is what refuses the document, so a
    // custom sink never sees output. The valid control runs the same sink, so
    // an empty read means "the sink was never given anything" rather than
    // "the sink never works".
    let refused_sink = RecordingJournal::new();
    let refused = FlowSpec::from_json(ENTRY_READ_BEFORE_WRITE).and_then(|flow| {
        Session::with_components(
            "review",
            flow,
            FixedResolver(UNRESOLVING),
            refused_sink.clone(),
        )
        .map(|_| ())
    });
    assert!(
        matches!(refused, Err(SessionError::VariableReadBeforeInit { .. })),
        "the document is refused by validation, before a session exists: {refused:?}"
    );
    assert!(
        refused_sink.records().is_empty(),
        "a refused document must reach no journal: {:?}",
        refused_sink.records()
    );

    let accepted_sink = RecordingJournal::new();
    let accepted = FlowSpec::from_json(
        r#"{
  "vars": {"name": "string"},
  "entry": "ask",
  "nodes": {
    "ask": {"ask": {
      "var": "name",
      "options": ["Ada"],
      "routes": {"Ada": "greet"}
    }},
    "greet": {"say": {"text": "Hello ${name}"}},
    "end": "end"
  },
  "edges": [{"next": {"from": "greet", "to": "end"}}],
  "bounds": {"budget": 8}
}"#,
    )
    .and_then(|flow| {
        Session::with_components(
            "review",
            flow,
            FixedResolver(Resolution::Resolved {
                index: 0,
                tier: MatchTier::Exact,
                score: 1.0,
                lead: 1.0,
            }),
            accepted_sink.clone(),
        )
        .map(|_| ())
    });
    accepted?;
    assert!(
        !accepted_sink.records().is_empty(),
        "the control flow must reach the sink, or the empty read above proves nothing"
    );
    Ok(())
}
