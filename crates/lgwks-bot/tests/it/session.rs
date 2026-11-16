//! Public acceptance tests for the synchronous flow/session surface.
//!
//! This file holds the builders and doubles more than one group uses. Each
//! behaviour has its own module: `flow_shape` (what a flow document may
//! contain), `resolution` (how an answer is resolved, and aliases),
//! `receipts` and `receipt_sink` (the decision record and where it goes),
//! `answers` (what an option stores and routes to), `terminals` (declared
//! outcomes against what a node carries), `limits` and `limits_session` (the
//! byte ceilings) and `definite_assignment` (reads before writes).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use lgwks_bot::semantic::{Embedder, EmbedderIdentity};
use lgwks_bot::session::{
    DecisionReceipt, FlowBounds, FlowSpec, Journal, JournalError, MemoryJournal, NodeKind,
    PolicyVersion, Provenance, Question, ReceiptAcceptance, Resolution, Resolver, SessionError,
    Terminal, VarType, Verdict,
};

mod answers;
mod definite_assignment;
mod flow_shape;
mod limits;
mod limits_session;
mod receipt_sink;
mod receipts;
mod resolution;
mod terminals;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A `Say` node speaking `text`.
fn say(text: &str) -> NodeKind {
    NodeKind::Say {
        text: String::from(text),
    }
}

/// An `Ask` node storing its answer in `var`, offering `options`, and routing
/// each `(option, node)` pair.
fn ask(var: &str, options: &[&str], routes: &[(&str, &str)]) -> NodeKind {
    NodeKind::Ask {
        var: String::from(var),
        options: options.iter().map(|option| String::from(*option)).collect(),
        routes: routes
            .iter()
            .map(|&(option, node)| (String::from(option), String::from(node)))
            .collect(),
    }
}

/// A `Refer` node pointing at `target` with `text`.
fn refer(target: &str, text: &str) -> NodeKind {
    NodeKind::Refer {
        target: String::from(target),
        text: String::from(text),
    }
}

/// A `Handoff` node to `target`.
fn handoff(target: &str) -> NodeKind {
    NodeKind::Handoff {
        target: String::from(target),
    }
}

/// A terminal outcome referring the session to `target`.
fn referred_to(target: &str) -> Terminal {
    Terminal::Referred {
        target: String::from(target),
    }
}

/// A terminal outcome handing the session off to `target`.
fn handed_off_to(target: &str) -> Terminal {
    Terminal::HandedOff {
        target: String::from(target),
    }
}

/// A terminal outcome refusing the session for `reason`.
fn refused_for(reason: &str) -> Terminal {
    Terminal::Refused {
        reason: String::from(reason),
    }
}

/// A choice variable over `options`.
fn choice(options: &[&str]) -> VarType {
    VarType::Choice(options.iter().map(|option| String::from(*option)).collect())
}

/// Build a single terminal flow.
fn terminal_flow(kind: NodeKind, terminal: Option<Terminal>) -> Result<FlowSpec, SessionError> {
    let terminals = match terminal {
        Some(outcome) => BTreeMap::from([(String::from("start"), outcome)]),
        None => BTreeMap::new(),
    };
    FlowSpec::new(
        BTreeMap::new(),
        "start",
        BTreeMap::from([(String::from("start"), kind)]),
        Vec::new(),
        terminals,
        FlowBounds::new(8),
    )
}

/// The policy version a [`FixedResolver`] attributes its verdicts to.
///
/// Constant rather than freshly digested per resolver: these resolvers return a
/// scripted resolution, so what varies between them is the resolution and never
/// the provenance. A test that needs the provenance to vary constructs its own
/// [`Verdict`] instead of reaching through this helper.
fn fixed_policy() -> PolicyVersion {
    PolicyVersion::new("test-fixed", &[1.0])
}

/// A resolver that always reports the same verdict, so a session's behaviour
/// can be observed against each [`Resolution`] variant in isolation.
struct FixedResolver(Resolution);

impl Resolver for FixedResolver {
    fn resolve(&self, _utterance: &str, _question: &Question<'_>) -> Verdict {
        Verdict::new(self.0.clone(), Provenance::without_model(fixed_policy()))
    }
}

/// One `Ask` node whose options route to an `End`, so a resolved answer is
/// observable as the session leaving the node.
fn one_question_flow() -> Result<FlowSpec, SessionError> {
    let mut vars = BTreeMap::new();
    vars.insert(String::from("choice"), choice(&["yes", "no"]));
    FlowSpec::new(
        vars,
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                ask("choice", &["yes", "no"], &[("yes", "done"), ("no", "done")]),
            ),
            (String::from("done"), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(8),
    )
}

/// One `Ask` node over a single variable, routing every option to an `End`.
fn single_ask_flow(declared: VarType, options: Vec<String>) -> Result<FlowSpec, SessionError> {
    let routes = options
        .iter()
        .map(|option| (option.clone(), String::from("done")))
        .collect();
    FlowSpec::new(
        BTreeMap::from([(String::from("answer"), declared)]),
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                NodeKind::Ask {
                    var: String::from("answer"),
                    options,
                    routes,
                },
            ),
            (String::from("done"), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(8),
    )
}

/// One `Ask` node whose options are ordinary English, so a phrase that shares no
/// letter with either of them is the only way to reach the semantic tier.
fn order_flow() -> Result<FlowSpec, SessionError> {
    order_ask_flow(["Repeat last order", "Cancel"])
}

/// One `Ask` over the two order options, offered in `options`' order, each
/// routed to a shared `End`.
fn order_ask_flow(options: [&str; 2]) -> Result<FlowSpec, SessionError> {
    FlowSpec::new(
        BTreeMap::from([(String::from("order"), choice(&options))]),
        "ask",
        BTreeMap::from([
            (
                String::from("ask"),
                ask(
                    "order",
                    &options,
                    &[("Repeat last order", "end"), ("Cancel", "end")],
                ),
            ),
            (String::from("end"), NodeKind::End),
        ]),
        Vec::new(),
        BTreeMap::new(),
        FlowBounds::new(8),
    )
}

/// Whether every embedding an [`OrderEmbedder`] returns carries a direction.
///
/// A named type rather than a `bool` on the embedder, so the test that reads it
/// says which state it is asking for and the compiler checks that the two
/// states are the two that exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measurability {
    /// Every embedding is a direction, so every comparison that is attempted
    /// produces a score.
    Complete,
    /// One competitor embeds to the zero vector: the right width, so the
    /// resolver's dimension check accepts it, and no direction, so no angle
    /// exists between it and anything else.
    DegenerateCompetitor,
}

/// An embedder that places `"the usual"` next to `"Repeat last order"` and
/// everything else orthogonally, so the test's geometry is its assertion.
struct OrderEmbedder {
    identity: EmbedderIdentity,
    measurability: Measurability,
}

impl Embedder for OrderEmbedder {
    type Error = std::convert::Infallible;

    fn identity(&self) -> &EmbedderIdentity {
        &self.identity
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, Self::Error> {
        let vector = match (text, self.measurability) {
            ("the usual" | "Repeat last order", _) => vec![1.0, 0.0],
            ("Cancel", Measurability::DegenerateCompetitor) => vec![0.0, 0.0],
            _ => vec![0.0, 1.0],
        };
        Ok(vector)
    }
}

/// One journal record: the path node, the role label, and the rendered text.
type JournalRecord = (String, String, String);

/// The buffer a [`SharedJournal`] writes into, and the handle a test reads.
type JournalRecords = Rc<RefCell<Vec<JournalRecord>>>;

/// A journal whose records the caller keeps a handle to.
///
/// A session owns its journal, so a test that inspects the session after a
/// refused construction has nothing left to inspect: the error path returns no
/// session. Sharing the record buffer instead makes "refused before anything
/// was written" observable rather than asserted.
#[derive(Clone)]
struct SharedJournal {
    records: JournalRecords,
    /// Where the receipts go.
    ///
    /// This double exists to observe the path records, and delegating the
    /// receipts to the crate's own in-memory sink rather than a second
    /// hand-rolled store is what keeps the two from drifting. Delegated rather
    /// than accepted-and-dropped because a sink that drops a receipt and
    /// reports success is the failure the [`Journal`] contract names in as many
    /// words.
    receipts: MemoryJournal,
}

impl SharedJournal {
    /// An empty journal and the handle that reads it.
    fn new() -> (Self, JournalRecords) {
        let records: JournalRecords = Rc::new(RefCell::new(Vec::new()));
        (
            Self {
                records: Rc::clone(&records),
                receipts: MemoryJournal::new(),
            },
            records,
        )
    }
}

impl Journal for SharedJournal {
    fn record(&mut self, path_node: &str, role: &str, text: &str) {
        self.records
            .borrow_mut()
            .push((path_node.to_owned(), role.to_owned(), text.to_owned()));
    }

    /// Forwarded whole to [`MemoryJournal`], which answers `InMemory` and says
    /// why in its own implementation.
    fn record_decision(
        &mut self,
        receipt: &DecisionReceipt,
    ) -> Result<ReceiptAcceptance, JournalError> {
        self.receipts.record_decision(receipt)
    }
}
