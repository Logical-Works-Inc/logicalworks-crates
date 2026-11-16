//! Where a decision record goes: a refused receipt is not an accepted answer,
//! and the sink says whether a receipt outlives the process.

use lgwks_bot::semantic::SemanticResolver;
use lgwks_bot::session::{Session, Value};

use super::receipts::{TestJournal, measured};
use super::*;

#[test]
fn a_refused_receipt_is_not_an_accepted_answer() -> TestResult {
    let journal = TestJournal::refusing(ReceiptAcceptance::Durable, 1);
    let mut session = Session::with_components(
        "refused",
        order_flow()?,
        SemanticResolver::new(measured("digest-a")?),
        journal.clone(),
    )?;

    let refusal = session.answer("the usual");
    let rendered = format!("{refusal:?}");
    let Err(SessionError::ReceiptNotRecorded { node, cause }) = refusal else {
        return Err(format!("expected a receipt refusal, got {rendered}").into());
    };
    assert_eq!(
        node, "ask",
        "the refusal names the node whose decision it was"
    );
    assert_eq!(
        cause.sink(),
        "test-sink",
        "the refusal names the sink that refused"
    );
    assert_eq!(
        cause.cause(),
        "the sink refused the write",
        "and carries the sink's own description of the failure"
    );

    // The answer was not taken. Every observable the session has says so.
    assert_eq!(session.current(), Some("ask"), "the cursor did not move");
    assert_eq!(session.terminal(), None, "the session did not complete");
    assert_eq!(
        session.scope().get("order"),
        None,
        "the variable was not written"
    );
    assert!(
        session.decisions().is_empty(),
        "the session does not hold a receipt the sink never took"
    );
    assert!(
        !session
            .transcript()
            .iter()
            .any(|entry| entry.text() == "the usual"),
        "an answer the session did not accept is not recorded as having been said"
    );
    assert_eq!(
        journal.refused().len(),
        1,
        "the sink did refuse one receipt"
    );
    assert!(journal.accepted().is_empty(), "and accepted none");

    // The refusal is not a dead end: the same answer is taken once the sink
    // takes it.
    session.answer("the usual")?;
    assert_eq!(
        session.terminal(),
        Some(&Terminal::Completed),
        "the session advanced once the receipt was taken"
    );
    assert_eq!(
        session.scope().get("order"),
        Some(&Value::Choice(String::from("Repeat last order")))
    );
    assert_eq!(session.decisions().len(), 1, "and holds the receipt");
    assert_eq!(journal.accepted().len(), 1, "which the sink took");
    Ok(())
}

#[test]
fn the_sink_says_whether_a_receipt_outlives_the_process() -> TestResult {
    // One session identity, one flow document, one utterance, two sinks. The
    // only thing that differs between the runs is what the sink does with the
    // receipt, so any difference in the record is the sink's doing.
    let mut in_memory = Session::with_resolver(
        "same",
        order_flow()?,
        SemanticResolver::new(measured("digest-a")?),
    )?;
    in_memory.answer("the usual")?;

    let mut durable = Session::with_components(
        "same",
        order_flow()?,
        SemanticResolver::new(measured("digest-a")?),
        TestJournal::accepting(ReceiptAcceptance::Durable),
    )?;
    durable.answer("the usual")?;

    let held = in_memory
        .decisions()
        .first()
        .ok_or_else(|| String::from("the in-memory run recorded no receipt"))?;
    let written = durable
        .decisions()
        .first()
        .ok_or_else(|| String::from("the durable run recorded no receipt"))?;

    assert_eq!(
        held.acceptance(),
        ReceiptAcceptance::InMemory,
        "a receipt held in the session's own journal is gone when the process is"
    );
    assert_eq!(
        written.acceptance(),
        ReceiptAcceptance::Durable,
        "and a sink that writes it through says so"
    );
    // The two sinks were handed the same record. Only the answer about
    // persistence differs, which is why that answer is stored beside the receipt
    // rather than inside it: one field of the receipt would then differ between
    // two sinks that were given identical bytes.
    assert_eq!(
        held.receipt(),
        written.receipt(),
        "both sinks were handed the same decision record"
    );
    Ok(())
}
