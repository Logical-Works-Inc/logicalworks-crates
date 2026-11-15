//! The decision record (#52): a resolution carries provenance, so two runs that
//! reach one verdict by different means leave different records.
//!
//! The defect these pin: a resolution carried no provenance, so two runs that
//! reached the same verdict by different means — a lexicon versus a model, one
//! weight set versus another, one threshold versus a retuned one — left records
//! that were identical. The tier and the score were reported; *on whose
//! authority* was not, and the session transcript cannot hold it, because a
//! transcript is a rendering of what was said and not of what was decided.

use std::cell::Cell;

use lgwks_bot::{
    DegradedReason, MatchTier, RECEIPT_VERSION, SemanticPolicy, SemanticResolver, Session, Value,
};

use super::*;

/// A journal that keeps every receipt it is handed, and can be told to refuse.
///
/// The state is behind `Rc` because `Session` owns its journal for the session's
/// lifetime and offers no route back to it — which is right for a sink, and the
/// reason this double is shared with the test rather than read back out of the
/// session.
#[derive(Clone)]
pub(super) struct TestJournal {
    /// Receipts this sink accepted, in the order it accepted them.
    accepted: Rc<RefCell<Vec<DecisionReceipt>>>,
    /// Receipts this sink refused, in the order it refused them.
    refused: Rc<RefCell<Vec<DecisionReceipt>>>,
    /// How many further receipts to refuse before accepting again.
    refusals: Rc<Cell<usize>>,
    /// What this sink reports when it accepts one.
    acceptance: ReceiptAcceptance,
}

impl TestJournal {
    /// A sink that accepts everything and reports `acceptance`.
    pub(super) fn accepting(acceptance: ReceiptAcceptance) -> Self {
        Self {
            accepted: Rc::new(RefCell::new(Vec::new())),
            refused: Rc::new(RefCell::new(Vec::new())),
            refusals: Rc::new(Cell::new(0)),
            acceptance,
        }
    }

    /// A sink that refuses its next `refusals` receipts and then accepts.
    pub(super) fn refusing(acceptance: ReceiptAcceptance, refusals: usize) -> Self {
        let sink = Self::accepting(acceptance);
        sink.refusals.set(refusals);
        sink
    }

    /// Returns the receipts this sink accepted.
    pub(super) fn accepted(&self) -> Vec<DecisionReceipt> {
        self.accepted.borrow().clone()
    }

    /// Returns the receipts this sink refused.
    pub(super) fn refused(&self) -> Vec<DecisionReceipt> {
        self.refused.borrow().clone()
    }
}

impl Journal for TestJournal {
    fn record(&mut self, _path_node: &str, _role: &str, _text: &str) {
        // This double exists for the receipt channel. Path records are the
        // transcript's copy of the same conversation, and every assertion about
        // them is made through `Session::transcript`.
    }

    fn record_decision(
        &mut self,
        receipt: &DecisionReceipt,
    ) -> Result<ReceiptAcceptance, JournalError> {
        if self.refusals.get() > 0 {
            self.refusals.set(self.refusals.get().saturating_sub(1));
            self.refused.borrow_mut().push(receipt.clone());
            let refusal = Err(JournalError::new("test-sink", "the sink refused the write"));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "record_decision: returning an error to the caller");
            return refusal;
        }
        self.accepted.borrow_mut().push(receipt.clone());
        Ok(self.acceptance)
    }
}

/// The tier a resolution was reached at, or `None` when it did not resolve.
///
/// The wildcard arm is reachable by a variant this crate adds later, and reports
/// `None`: a tier this test does not know is not a tier it may claim.
fn tier_of(resolution: &Resolution) -> Option<MatchTier> {
    match *resolution {
        Resolution::Resolved { tier, .. } => Some(tier),
        _ => None,
    }
}

/// The model the semantic tests below drive, fully measurable, under the
/// identity `digest` names.
pub(super) fn measured(digest: &str) -> Result<OrderEmbedder, Box<dyn std::error::Error>> {
    Ok(OrderEmbedder {
        identity: test_identity(digest)?,
        measurability: Measurability::Complete,
    })
}

/// The identity of the model the semantic tests below drive.
fn test_identity(digest: &str) -> Result<EmbedderIdentity, Box<dyn std::error::Error>> {
    Ok(EmbedderIdentity::new("test-model", digest, 2)?)
}

/// Runs `order_flow` for one utterance through a semantic resolver and returns
/// the session plus the one receipt it recorded.
///
/// Uses the shipped policy; see [`receipt_for_policy`] to vary it.
fn receipt_for(
    utterance: &str,
    embedder: OrderEmbedder,
) -> Result<(Session, DecisionReceipt), Box<dyn std::error::Error>> {
    receipt_for_policy(utterance, embedder, SemanticPolicy::DEFAULT)
}

/// As [`receipt_for`], with the tier's acceptance policy chosen by the caller.
fn receipt_for_policy(
    utterance: &str,
    embedder: OrderEmbedder,
    policy: SemanticPolicy,
) -> Result<(Session, DecisionReceipt), Box<dyn std::error::Error>> {
    let journal = TestJournal::accepting(ReceiptAcceptance::Durable);
    let mut session = Session::with_components(
        "receipt",
        order_flow()?,
        SemanticResolver::with_policy(embedder, policy),
        journal,
    )?;
    session.answer(utterance)?;
    let receipt = session
        .decisions()
        .first()
        .map(|entry| entry.receipt().clone())
        .ok_or_else(|| String::from("an accepted answer recorded no decision receipt"))?;
    Ok((session, receipt))
}

/// One `Ask` node whose options are `order_flow`'s in the opposite order, so a
/// selected option occupies a different index.
fn reversed_order_flow() -> Result<FlowSpec, SessionError> {
    order_ask_flow(["Cancel", "Repeat last order"])
}

#[test]
fn a_receipt_names_the_model_only_when_a_model_decided() -> TestResult {
    let (_, lexical) = receipt_for("Cancel", measured("digest-a")?)?;
    let (_, semantic) = receipt_for("the usual", measured("digest-a")?)?;

    // "Cancel" is one of the options verbatim, so the lexicon answered it and no
    // model was consulted. Naming a model here would attribute a reproducible
    // verdict to a model that never ran.
    assert_eq!(
        lexical.provenance().model(),
        None,
        "a lexicon verdict must not name a model"
    );
    assert_eq!(
        semantic.provenance().model().map(EmbedderIdentity::name),
        Some("test-model"),
        "a verdict only the model could reach must name the model"
    );
    assert_eq!(
        semantic.provenance().model().map(EmbedderIdentity::digest),
        Some("digest-a"),
        "a name alone cannot tell two weight sets apart"
    );
    assert_eq!(
        semantic
            .provenance()
            .model()
            .map(EmbedderIdentity::dimension),
        Some(2),
        "and the width is what makes two scores comparable at all"
    );

    // The two decisions differ in provenance and in nothing else the record
    // holds: one flow, one node, one candidate list, one session.
    assert_eq!(lexical.flow(), semantic.flow(), "one flow revision");
    assert_eq!(lexical.node(), semantic.node(), "one ask node");
    assert_eq!(lexical.session(), semantic.session(), "one session");
    assert_eq!(
        lexical.options(),
        semantic.options(),
        "one candidate list was offered to both"
    );
    assert_ne!(
        tier_of(lexical.resolution()),
        tier_of(semantic.resolution()),
        "one resolved at the lexical tier and the other at the semantic one"
    );
    Ok(())
}

#[test]
fn the_policy_that_decided_is_named_on_the_verdict_it_produced() -> TestResult {
    let (_, lexical) = receipt_for("Cancel", measured("digest-a")?)?;
    let (_, semantic) = receipt_for("the usual", measured("digest-a")?)?;

    // Every verdict names a policy, including the one whose tier consulted no
    // model: "the lexicon decided this" is itself a claim about a rule, and a
    // receipt that named no rule could not be read against a later release.
    assert_eq!(
        lexical.provenance().policy().label(),
        "lexicon",
        "the tier that decided is the tier whose policy is named"
    );
    assert_eq!(
        semantic.provenance().policy().label(),
        "semantic",
        "the semantic tier's own parameters are the ones in force"
    );
    assert_ne!(
        lexical.provenance().policy(),
        semantic.provenance().policy(),
        "two independently tuned tiers are two policies"
    );
    assert!(
        !lexical.provenance().policy().revision().is_empty(),
        "a policy version with no revision is not a version"
    );
    Ok(())
}

#[test]
fn two_models_that_score_identically_are_distinguishable_in_the_record() -> TestResult {
    let (first_session, first) = receipt_for("the usual", measured("digest-a")?)?;
    let (second_session, second) = receipt_for("the usual", measured("digest-b")?)?;

    // The two runs are indistinguishable everywhere a two-valued record looked:
    // same conversation, same verdict, same chosen text, same route.
    assert_eq!(
        first_session.transcript(),
        second_session.transcript(),
        "the same words produced the same transcript"
    );
    assert_eq!(first.resolution(), second.resolution(), "the same geometry");
    assert_eq!(first.selected(), second.selected(), "the same option text");
    assert_eq!(first.route(), second.route(), "the same transition");

    // And they are different runs, which only the model's own identity can say.
    assert_eq!(
        first.provenance().model().map(EmbedderIdentity::name),
        second.provenance().model().map(EmbedderIdentity::name),
        "both runs name the same model"
    );
    assert_ne!(
        first.provenance().model().map(EmbedderIdentity::digest),
        second.provenance().model().map(EmbedderIdentity::digest),
        "same name, different weights, different runs"
    );
    Ok(())
}

#[test]
fn a_retuned_policy_is_visible_when_the_resolution_is_unchanged() -> TestResult {
    let (_, shipped) =
        receipt_for_policy("the usual", measured("digest-a")?, SemanticPolicy::DEFAULT)?;
    let (_, retuned) = receipt_for_policy(
        "the usual",
        measured("digest-a")?,
        SemanticPolicy::new(0.70, 0.05)?,
    )?;

    assert_eq!(
        shipped.resolution(),
        retuned.resolution(),
        "the same answer was reached under both thresholds"
    );
    assert_eq!(
        shipped.provenance().model().map(EmbedderIdentity::digest),
        retuned.provenance().model().map(EmbedderIdentity::digest),
        "and against the same weights"
    );
    assert_ne!(
        shipped.provenance().policy().revision(),
        retuned.provenance().policy().revision(),
        "a different threshold is a different policy, and the revision says so"
    );
    Ok(())
}

#[test]
fn reordering_the_options_changes_the_identity_and_not_the_choice() -> TestResult {
    let (_, ordered) = receipt_for("the usual", measured("digest-a")?)?;

    let journal = TestJournal::accepting(ReceiptAcceptance::Durable);
    let mut reversed = Session::with_components(
        "reversed",
        reversed_order_flow()?,
        SemanticResolver::new(measured("digest-a")?),
        journal,
    )?;
    reversed.answer("the usual")?;
    let reversed_receipt = reversed
        .decisions()
        .first()
        .map(|entry| entry.receipt().clone())
        .ok_or_else(|| String::from("the reversed run recorded no receipt"))?;

    // The same text was chosen and the same node was reached, so a record that
    // kept only the chosen text would call these two runs identical.
    assert_eq!(
        ordered.selected(),
        reversed_receipt.selected(),
        "the same option text was chosen from both lists"
    );
    assert_eq!(
        ordered.route(),
        reversed_receipt.route(),
        "and both route to the same node"
    );

    // They are not the same question. The option-set identity digests the
    // ordered list, and the verdict's index is a position within it.
    assert_ne!(
        ordered.options(),
        reversed_receipt.options(),
        "a reordering is a different candidate list"
    );
    assert_ne!(
        ordered.resolution(),
        reversed_receipt.resolution(),
        "an index is a position in a list, and the position moved"
    );
    Ok(())
}

#[test]
fn a_receipt_exists_for_an_answer_that_did_not_resolve() -> TestResult {
    let journal = TestJournal::accepting(ReceiptAcceptance::InMemory);
    let mut session = Session::with_components(
        "unresolved",
        one_question_flow()?,
        FixedResolver(Resolution::Ambiguous {
            tied: vec![0, 1],
            tier: MatchTier::Fuzzy,
            score: 0.6,
        }),
        journal.clone(),
    )?;
    session.answer("maybe")?;

    assert_eq!(
        session.current(),
        Some("ask"),
        "an ambiguous answer re-asks"
    );
    assert_eq!(
        session.decisions().len(),
        1,
        "a question that was re-asked is still a decision, so it is still recorded"
    );
    let recorded = session
        .decisions()
        .first()
        .map(|entry| entry.receipt().clone())
        .ok_or_else(|| String::from("the re-ask recorded no receipt"))?;

    assert_eq!(recorded.selected(), None, "nothing was selected");
    assert_eq!(recorded.route(), None, "and nothing was routed");
    assert_eq!(
        recorded.resolution(),
        &Resolution::Ambiguous {
            tied: vec![0, 1],
            tier: MatchTier::Fuzzy,
            score: 0.6,
        },
        "the verdict is carried verbatim, including the tie"
    );
    assert_eq!(
        journal.accepted().len(),
        1,
        "the sink holds the record of the question that had to be asked again"
    );
    Ok(())
}

#[test]
fn a_receipt_binds_itself_to_the_flow_revision_that_produced_it() -> TestResult {
    let (session, receipt) = receipt_for("the usual", measured("digest-a")?)?;

    assert_eq!(
        receipt.flow(),
        session.flow_revision(),
        "the receipt names the digest of the flow document it came from"
    );

    // Two flows with different structure are two revisions, so a receipt cannot
    // be read against the wrong document.
    let other = Session::new("other", one_question_flow()?)?;
    assert_ne!(
        receipt.flow(),
        other.flow_revision(),
        "a different flow document is a different revision"
    );
    Ok(())
}

#[test]
fn a_receipt_is_self_describing_when_exported() -> TestResult {
    let (_, receipt) = receipt_for("the usual", measured("digest-a")?)?;

    assert_eq!(
        receipt.version(),
        RECEIPT_VERSION,
        "the record declares the schema that wrote it"
    );

    let exported = lgwks_bot::json::to_string(&receipt)?;
    for field in [
        "version",
        "session",
        "flow",
        "node",
        "options",
        "resolution",
        "provenance",
        "selected",
        "route",
    ] {
        assert!(
            exported.contains(field),
            "the exported record names its {field} field: {exported}"
        );
    }

    let reimported: DecisionReceipt = lgwks_bot::json::from_str(&exported)?;
    assert_eq!(
        reimported, receipt,
        "a record that does not survive the trip out of its own process is not a record"
    );
    Ok(())
}

/// Runs `one_question_flow` for one scripted verdict and returns the session
/// plus the receipt it recorded.
fn receipt_for_verdict(
    resolution: Resolution,
) -> Result<(Session, DecisionReceipt), Box<dyn std::error::Error>> {
    let journal = TestJournal::accepting(ReceiptAcceptance::InMemory);
    let mut session = Session::with_components(
        "outcomes",
        one_question_flow()?,
        FixedResolver(resolution),
        journal,
    )?;
    session.answer("yes")?;
    let receipt = session
        .decisions()
        .first()
        .map(|entry| entry.receipt().clone())
        .ok_or_else(|| String::from("the decision recorded no receipt"))?;
    Ok((session, receipt))
}

#[test]
fn the_four_verdicts_are_four_distinguishable_receipts() -> TestResult {
    let (resolved_session, resolved) = receipt_for_verdict(Resolution::Resolved {
        index: 0,
        tier: MatchTier::Fuzzy,
        score: 0.81,
        lead: 0.22,
    })?;
    let (_, ambiguous) = receipt_for_verdict(Resolution::Ambiguous {
        tied: vec![0, 1],
        tier: MatchTier::Fuzzy,
        score: 0.6,
    })?;
    let (_, absent) = receipt_for_verdict(Resolution::Absent { best_score: 0.1 })?;
    let (_, degraded) = receipt_for_verdict(Resolution::Degraded {
        reason: DegradedReason::EmbedderUnavailable,
    })?;

    // The verdict is carried verbatim: the tier, the score and the lead survive,
    // not only the index the session needed.
    assert_eq!(
        resolved.resolution(),
        &Resolution::Resolved {
            index: 0,
            tier: MatchTier::Fuzzy,
            score: 0.81,
            lead: 0.22,
        },
        "a record holding the index alone cannot say what produced it"
    );
    assert_eq!(
        absent.resolution(),
        &Resolution::Absent { best_score: 0.1 },
        "the best score observed is kept"
    );
    assert_eq!(
        degraded.resolution(),
        &Resolution::Degraded {
            reason: DegradedReason::EmbedderUnavailable,
        },
        "and a degraded verdict carries no score at all"
    );

    // Four verdicts, four different records. "Nothing was close" and "nothing
    // was compared" are the two a two-valued verdict renders identically, and
    // they need different repairs.
    assert_ne!(
        absent.resolution(),
        degraded.resolution(),
        "incomplete matching and an unavailable dependency are different records"
    );
    assert_ne!(resolved.resolution(), ambiguous.resolution());
    assert_ne!(ambiguous.resolution(), absent.resolution());

    // Only the resolved verdict selected a route, and what the receipt says it
    // selected is what the session actually stored.
    assert_eq!(resolved.selected(), Some("yes"), "the chosen option's text");
    assert_eq!(resolved.route(), Some("done"), "the node it routed to");
    assert_eq!(
        resolved_session.scope().get("choice"),
        Some(&Value::Choice(String::from("yes"))),
        "the receipt's selection agrees with the value the session stored"
    );
    assert_eq!(
        resolved_session.terminal(),
        Some(&Terminal::Completed),
        "and with where the session ended up"
    );
    for (label, receipt) in [
        ("ambiguous", &ambiguous),
        ("absent", &absent),
        ("degraded", &degraded),
    ] {
        assert_eq!(
            receipt.selected(),
            None,
            "a {label} verdict selected nothing, and says so by absence"
        );
        assert_eq!(receipt.route(), None, "and routed nowhere");
    }
    Ok(())
}
