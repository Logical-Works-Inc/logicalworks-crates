//! Outcome receipts and unreadable journals: a receipt for the wrong position
//! settles nothing, and a read that failed is not an absent outcome
//! (issues #123 and #100).

use super::scripted_journal::{Receipts, ScriptedJournal};
use super::*;

/// One external action invocation.
struct ExternalCount(Rc<Cell<usize>>);

impl Execute for ExternalCount {
    type Input = EventId<u32>;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::External
    }

    async fn execute_action(&self, call: (Auth, &EventId<u32>)) -> Result<Self::Output, EcsError> {
        call.0.check(Execute::required_caps(self))?;
        self.0.set(self.0.get().saturating_add(1));
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::external_count"
    }
}

/// An unrelated durable receipt must not settle an outcome or authorize a
/// recovered action to run again.
#[test]
fn an_unrelated_outcome_receipt_is_refused_without_reentering_the_effect() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let receipt = Rc::new(Cell::new(DurabilityPromise::ProcessCrash));
    let unrelated_receipt = Rc::new(Cell::new(true));
    let unrelated_append_ack = Rc::new(Cell::new(true));
    let seen = Rc::new(RefCell::new(EventId::new(1, 1)));
    let entered = Rc::new(Cell::new(0));
    let identity = identity()?;
    let environment = identity.environment();
    let mut run_broker = Broker::new();
    run_broker.register(environment)?;

    let mut bot = every_request(
        Rc::clone(&seen),
        ExternalCount(Rc::clone(&entered)),
        EffectScope::new(
            identity,
            run_broker,
            Box::new(
                ScriptedJournal::over(Rc::clone(&store))
                    .advertising(DurabilityPromise::ProcessCrash)
                    .with_entries()
                    .receipting(Receipts {
                        receipt: Rc::clone(&receipt),
                        unrelated_receipt: Rc::clone(&unrelated_receipt),
                        unrelated_append_ack: Rc::clone(&unrelated_append_ack),
                    }),
            ),
        ),
    )?;

    match bot.tick() {
        Err(EcsError::EffectUnrecorded { cause, .. }) => assert!(
            matches!(
                *cause,
                DispatchError::Journal(JournalError::ReceiptMismatch { .. })
            ),
            "an unrelated receipt must be rejected as an identity mismatch, got {cause:?}"
        ),
        other => return Err(format!("expected receipted recording failure, got {other:?}").into()),
    }
    assert_eq!(entered.get(), 1, "the external action ran exactly once");
    drop(bot);

    let mut recovered = every_request(
        Rc::clone(&seen),
        ExternalCount(Rc::clone(&entered)),
        EffectScope::new(
            identity,
            broker(identity.environment())?,
            Box::new(
                ScriptedJournal::over(store)
                    .advertising(DurabilityPromise::ProcessCrash)
                    .with_entries()
                    .receipting(Receipts {
                        receipt,
                        unrelated_receipt: Rc::clone(&unrelated_receipt),
                        unrelated_append_ack,
                    }),
            ),
        ),
    )?;

    let held = recovered.pending();
    assert_eq!(held.len(), 1, "the unverified durable outcome remains held");
    assert!(matches!(
        held[0].hold(),
        TransitionHold::RecordingFailed { .. }
    ));
    let key = held[0]
        .key()
        .ok_or("the recovered receipt hold has a key")?;
    unrelated_receipt.set(false);
    recovered.resolve_effect(&key, EffectEvidence::Applied)?;
    assert!(
        recovered.pending().is_empty(),
        "the bound receipt clears the hold"
    );
    assert_eq!(
        entered.get(),
        1,
        "recovery did not re-enter the external action"
    );
    Ok(())
}

/// An unreadable journal is not an absent outcome (issue #123).
///
/// A settled and reconstructed bot, a transient `committed()`-only failure, and
/// the same Applied evidence again: the answer must be a journal-read refusal
/// with nothing mutated and no append, never `NoSuchWork`. Once reads come
/// back, the same request is the idempotent duplicate it always was.
#[test]
fn a_transient_journal_read_failure_is_not_reported_as_no_such_work() -> TestResult {
    let watch = Watch::new()?;

    let mut first = watch.bot()?;
    assert_eq!(first.tick()?, 1, "the episode runs and settles Applied");
    assert_eq!(watch.counted.get(), 1);
    drop(first);

    let events = recorded(&watch.store)?;
    let key = events
        .iter()
        .find_map(|event| match *event {
            EffectEvent::OutcomeObserved { key, .. } => Some(key),
            _ => None,
        })
        .ok_or("the first episode recorded an outcome")?;
    let appends_before = events.len();

    let fail_reads = Rc::new(Cell::new(false));
    let mut second = watch.bot_with(EffectScope::new(
        watch.identity,
        broker(watch.identity.environment())?,
        Box::new(
            ScriptedJournal::over(Rc::clone(&watch.store))
                .with_entries()
                .failing_reads(Rc::clone(&fail_reads)),
        ),
    ))?;

    // The transient failure starts after assembly: the counterexample is a
    // read that becomes unavailable under an established identity and key.
    fail_reads.set(true);
    match second.resolve_effect(&key, EffectEvidence::Applied) {
        Err(EcsError::Kernel(KernelError::EffectRefused {
            cause: DispatchError::Journal(_),
        })) => {}
        other => {
            return Err(format!(
                "an unreadable journal must be a journal-read refusal, never NoSuchWork: {other:?}"
            )
            .into());
        }
    }
    assert_eq!(
        recorded(&watch.store)?.len(),
        appends_before,
        "a read failure appends nothing"
    );
    assert_eq!(
        watch.counted.get(),
        1,
        "a read failure never re-enters the action"
    );
    assert!(
        second.pending().is_empty(),
        "a read failure does not drop a held obligation or invent one"
    );

    // Reads come back. The same request is a legitimate idempotent duplicate.
    fail_reads.set(false);
    match second.resolve_effect(&key, EffectEvidence::Applied) {
        Ok(()) => {}
        other => {
            return Err(format!(
                "with reads restored, the repeat is an idempotent success: {other:?}"
            )
            .into());
        }
    }
    assert_eq!(
        recorded(&watch.store)?.len(),
        appends_before,
        "no second append"
    );
    assert_eq!(watch.counted.get(), 1, "still no re-entry");
    Ok(())
}

/// A readable journal with no such outcome is still `NoSuchWork` (issue #123).
///
/// The control that stops a blanket "always retryable" answer from passing: the
/// two cases must stay distinguishable from outside the crate.
#[test]
fn a_readable_absent_outcome_is_still_no_such_work() -> TestResult {
    let watch = Watch::new()?;

    let mut first = watch.bot()?;
    assert_eq!(first.tick()?, 1);
    drop(first);

    let events = recorded(&watch.store)?;
    let landed = events
        .iter()
        .find_map(|event| match *event {
            EffectEvent::OutcomeObserved { key, .. } => Some(key),
            _ => None,
        })
        .ok_or("the first episode recorded an outcome")?;
    // A key the journal never recorded an outcome for: same action, a later
    // attempt. The store is readable throughout.
    let never_recorded =
        landed.with_attempt(landed.attempt().checked_next().ok_or("attempt overflow")?);

    let mut second = watch.bot()?;

    match second.resolve_effect(&never_recorded, EffectEvidence::Applied) {
        Err(EcsError::NoSuchWork { .. }) => {}
        other => {
            return Err(format!("a readable absent outcome is NoSuchWork, got {other:?}").into());
        }
    }
    Ok(())
}

/// A readable outcome that contradicts the report is still `EvidenceContradicted`.
#[test]
fn a_readable_contradictory_outcome_is_still_contradicted() -> TestResult {
    let watch = Watch::new()?;

    let mut first = watch.bot()?;
    assert_eq!(first.tick()?, 1);
    drop(first);

    let key = recorded(&watch.store)?
        .iter()
        .find_map(|event| match *event {
            EffectEvent::OutcomeObserved { key, .. } => Some(key),
            _ => None,
        })
        .ok_or("the first episode recorded an outcome")?;

    let mut second = watch.bot()?;

    match second.resolve_effect(&key, EffectEvidence::NotApplied) {
        Err(EcsError::EvidenceContradicted {
            settled: EffectEvidence::Applied,
            ..
        }) => {}
        other => {
            return Err(
                format!("a readable contradiction is EvidenceContradicted, got {other:?}").into(),
            );
        }
    }
    Ok(())
}
