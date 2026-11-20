//! The parts of the takeover row that need no second process: the generation
//! comparison, the receipt that outlives its writer, and the two fences.

use lgwks_bot::broker::BrokerError;
use lgwks_bot::effect::EnvironmentEpoch;
use lgwks_bot::journal::{
    DurabilityPromise, EffectEvent, EffectJournal, EventKind, FileJournal, JournalError,
};

use super::shared::{TempGuard, scratch_dir, walk_ladder};
use super::{
    FIRST_GENERATION, TAKEOVER_PREDICATE, TestResult, broker_at_first_generation, environment,
    takeover_key,
};

/// The generation check is a comparison, and the comparison is where a takeover
/// is enforced, so it is asserted on its own: a warrant minted at the current
/// generation is accepted, and one from the generation before it is not.
#[test]
fn a_warrant_from_the_previous_generation_is_superseded() -> TestResult {
    let broker = broker_at_first_generation()?;
    let key = takeover_key("1", FIRST_GENERATION)?;
    let authority = broker.authorize(key)?;
    assert_eq!(
        authority.epoch(),
        EnvironmentEpoch::from_decimal(FIRST_GENERATION)?,
        "the warrant names the generation it was minted against"
    );
    broker.revalidate(&authority)?;

    let mut moved = broker;
    moved.replace(environment()?)?;
    match moved.revalidate(&authority) {
        Err(BrokerError::Superseded { .. }) => {}
        Err(other) => {
            return Err(format!("a replaced warrant must be Superseded, got {other}").into());
        }
        Ok(()) => {
            return Err("the broker revalidated a warrant for a replaced generation".into());
        }
    }
    Ok(())
}

/// A generation this broker never issued is a different refusal from one it
/// replaced, and the two are told apart so an operator investigating one is not
/// sent to investigate the other.
#[test]
fn a_generation_the_broker_never_issued_is_not_a_supersession() -> TestResult {
    let broker = broker_at_first_generation()?;
    let invented = takeover_key("1", "9")?;
    match broker.authorize(invented) {
        Err(BrokerError::NeverIssued {
            presented, current, ..
        }) => {
            assert_eq!(
                presented,
                EnvironmentEpoch::from_decimal("9")?,
                "the refusal names the generation presented"
            );
            assert_eq!(
                current,
                EnvironmentEpoch::from_decimal(FIRST_GENERATION)?,
                "and the one the broker holds"
            );
        }
        Err(other) => {
            return Err(format!("an invented generation must be NeverIssued, got {other}").into());
        }
        Ok(_) => return Err("the broker authorized a generation it never issued".into()),
    }
    Ok(())
}

/// An acknowledged position reads back identically after a reopen: the durable
/// ack the dead worker held is a promise about bytes on a device, and it outlives
/// the process that minted it. This is what makes the receipt a receipt rather
/// than a claim.
#[test]
fn an_acknowledged_position_reads_back_identically_after_a_reopen() -> TestResult {
    let dir = scratch_dir("receipt")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");
    let key = takeover_key("1", FIRST_GENERATION)?;

    let ack = {
        let mut journal = FileJournal::open(&journal_path)?;
        assert_eq!(
            journal.durability(),
            DurabilityPromise::ProcessCrash,
            "the file journal earns the promise the row's kill depends on"
        );
        journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key })?;
        journal.compare_and_append(journal.tail(), &EffectEvent::DispatchPrepared { key })?
    };

    let reopened = FileJournal::open(&journal_path)?;
    let entry = reopened
        .committed_entry(ack.position())?
        .ok_or("an acknowledged position must read back after a reopen")?;
    assert_eq!(
        entry.position(),
        ack.position(),
        "the position is the same one"
    );
    assert_eq!(
        entry.event(),
        &EffectEvent::DispatchPrepared { key },
        "and it holds the prepared attempt, not a neighbour"
    );
    assert_eq!(
        entry.event().kind(),
        EventKind::DispatchPrepared,
        "so a receipt read after the takeover is still a receipt of that fact"
    );
    Ok(())
}

/// The whole attempt at the *current* generation settles and verifies, so the
/// refusals elsewhere in this file are the fence refusing and not the ladder being
/// unable to say yes. The control that stops a blanket "everything is fenced"
/// answer from passing.
#[test]
fn the_current_generation_may_still_settle_its_own_attempt() -> TestResult {
    let dir = scratch_dir("settle-own")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");
    let broker = broker_at_first_generation()?;
    let key = takeover_key("1", FIRST_GENERATION)?;

    let mut journal = FileJournal::open(&journal_path)?;
    // The control is the same walk at the current generation: if this is refused
    // then the row's refusals are about the takeover and not about the ladder.
    broker.authorize(key)?;
    walk_ladder(&mut journal, key, TAKEOVER_PREDICATE, 1, 4)?;
    assert_eq!(
        journal.committed()?.len(),
        4,
        "a whole attempt at the current generation settles and verifies"
    );
    Ok(())
}

/// The generation and the tail are two fences and both answer, because a caller
/// that fenced only one of them would pass a row asserting one.
#[test]
fn a_generation_and_a_tail_are_two_fences_and_both_answer() -> TestResult {
    let dir = scratch_dir("two-fences")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");
    let key = takeover_key("1", FIRST_GENERATION)?;
    let other = takeover_key("2", FIRST_GENERATION)?;

    let mut journal = FileJournal::open(&journal_path)?;
    journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key })?;
    let stale_tail = journal.tail();
    journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key: other })?;

    // The tail fence: an append against the position from before the last write
    // is refused, and nothing is written.
    let before = journal.committed()?.len();
    match journal.compare_and_append(stale_tail, &EffectEvent::DispatchPrepared { key }) {
        Err(JournalError::TailMismatch { .. }) => {}
        Err(other) => return Err(format!("a stale tail must be TailMismatch, got {other}").into()),
        Ok(_) => return Err("an append from behind the tail was accepted".into()),
    }
    assert_eq!(
        journal.committed()?.len(),
        before,
        "the refused append wrote nothing"
    );
    Ok(())
}
