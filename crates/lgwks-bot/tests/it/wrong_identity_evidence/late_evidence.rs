//! T11: late evidence for an attempt that is over.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::ecs::EcsError;
use lgwks_bot::effect::EffectKey;
use lgwks_bot::error::KernelError;
use lgwks_bot::journal::{AttemptStatus, MemoryJournal};
use lgwks_bot::spec::{Bot, EffectEvidence as Settled};

use crate::sim::rig::recorded;

use super::{TestResult, held_under};

/// Deliver `evidence` for `stale` and require the stale-attempt refusal that
/// names `stale`'s own attempt and the one outstanding, with nothing moved.
fn refused_as_stale(
    bot: &mut Bot,
    stale: EffectKey,
    outstanding: EffectKey,
    evidence: Settled,
) -> Result<(), Box<dyn Error>> {
    match bot.resolve_effect(&stale, evidence) {
        Err(EcsError::EvidenceStaleAttempt {
            reported,
            outstanding: live,
            ..
        }) if reported == stale.attempt() && live == outstanding.attempt() => Ok(()),
        other => {
            let refusal = Err(format!(
                "late {evidence:?} for attempt {:?} must be refused as stale, naming it and the \
                 outstanding attempt {:?}; got {other:?}",
                stale.attempt(),
                outstanding.attempt()
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "refused_as_stale: returning an error to the caller");
            refusal
        }
    }
}

/// T11: A goes Unknown, is settled NotApplied, and its retry B goes Unknown;
/// A's evidence delivered late, whether a repeat of NotApplied or a
/// contradicting Applied, cannot alter B, and each refusal is attributed to A.
///
/// A and B are two attempts on one entry, which is the case the row is about:
/// NotApplied makes the entry eligible again, the retry is a new attempt with
/// its own key, and from then on A's evidence describes an attempt that is
/// over. Accepting the repeat as an idempotent success would be a lie about
/// which attempt it moved; accepting the Applied would retire B, an attempt
/// whose effect may be live, on evidence that was never about it.
///
/// Each is refused by the check that is about A. The repeat agrees with A's
/// durable record, so it is refused as `EvidenceStaleAttempt` carrying A's
/// attempt and B's. The Applied disagrees with that record, and the record is
/// keyed by A's attempt, so it is refused as `EvidenceContradicted` naming A's
/// `NotApplied`. Either way the journal does not move, B stays held under its
/// own key, and B's body is not entered again. The control is the last step:
/// B's own Applied, the same evidence A's refusal named as a contradiction,
/// still settles B, so the refusals were about A's key and not a frozen entry.
#[test]
fn late_evidence_for_a_settled_attempt_cannot_alter_its_retry_t11() -> TestResult {
    let held = held_under(Rc::new(RefCell::new(MemoryJournal::new())))?;
    let mut bot = held.bot;
    let attempt_a = held.binding;

    // A Unknown -> NotApplied: the entry is eligible for an attempt again.
    bot.resolve_effect(&attempt_a, Settled::NotApplied)?;
    // B Unknown: the retry runs the body once more and is held in turn.
    match bot.tick() {
        Err(EcsError::Kernel(KernelError::EffectIndeterminate { .. })) => {}
        other => {
            let refusal =
                Err(format!("the retry must be held as indeterminate, got {other:?}").into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "late_evidence_for_a_settled_attempt_cannot_alter_its_retry_t11: returning an error to the caller");
            return refusal;
        }
    }
    let attempt_b = bot
        .pending()
        .first()
        .ok_or("the retry must be held")?
        .key()
        .ok_or("a held retry carries its key")?;
    assert_ne!(
        attempt_b.attempt(),
        attempt_a.attempt(),
        "the retry is a new attempt, not attempt A again"
    );
    assert_eq!(
        held.entered.get(),
        2,
        "the body ran for A and once more for B"
    );
    let before = recorded(&held.store)?;

    // Delayed A NotApplied: a repeat of A's own answer, about an attempt that is over.
    refused_as_stale(&mut bot, attempt_a, attempt_b, Settled::NotApplied)?;
    // Delayed A Applied: a contradiction of A's own durable record.
    match bot.resolve_effect(&attempt_a, Settled::Applied) {
        Err(EcsError::EvidenceContradicted {
            settled: Settled::NotApplied,
            submitted: Settled::Applied,
            ..
        }) => {}
        other => {
            let refusal = Err(format!(
                "late Applied for attempt A must be refused as contradicting A's own NotApplied, \
                 got {other:?}"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "late_evidence_for_a_settled_attempt_cannot_alter_its_retry_t11: returning an error to the caller");
            return refusal;
        }
    }

    assert_eq!(
        recorded(&held.store)?,
        before,
        "late evidence for attempt A wrote to the journal"
    );
    let still = bot.pending();
    assert_eq!(still.len(), 1, "B is still the one held entry");
    assert_eq!(
        still.first().and_then(|work| work.key()),
        Some(attempt_b),
        "and it is still held under B's key"
    );
    assert_eq!(held.entered.get(), 2, "no refusal re-entered the body");
    let recovered = held.store.borrow().recover();
    assert_eq!(
        recovered.status(attempt_a),
        Some(AttemptStatus::NotApplied),
        "A keeps its own settlement"
    );
    assert_eq!(
        recovered.status(attempt_b),
        Some(AttemptStatus::OutcomeUnknown),
        "B stays Unknown"
    );

    // The control: B's own evidence still settles B.
    bot.resolve_effect(&attempt_b, Settled::Applied)?;
    assert!(
        bot.pending().is_empty(),
        "B's own evidence retires the entry"
    );
    assert_eq!(
        held.store.borrow().recover().status(attempt_b),
        Some(AttemptStatus::Applied),
        "and the journal records it against B"
    );
    Ok(())
}
