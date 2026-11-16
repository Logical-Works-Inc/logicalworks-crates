//! What a restart does with what it finds: hold, retire, retry, continue.

use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::KernelError;
use lgwks_bot::journal::EventKind;
use lgwks_bot::spec::{Bot, EffectEvidence};

use crate::band_family::band_family;
use crate::sim::{self, Band};

use super::{Run, TestResult};

/// A recovered unknown is held, not resent.
fn restart_holds(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        drop(run.never()?.tick());

        let mut second = run.never()?;
        match second.tick() {
            Err(EcsError::PendingTransition { .. }) => {}
            other => {
                let refusal = Err(format!(
                    "the first tick of a restart must hold the recovered attempt and say \
                 so; it returned {other:?}"
                )
                .into());
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "restart_holds: returning an error to the caller");
                return refusal;
            }
        }
        assert_eq!(
            run.entered(),
            1,
            "the restart held the attempt, so the body must not have run again"
        );
        sim.record("recovered-unknown-held");
        sim.trace
            .record_number("restart-entered", u64::from(run.entered()));
        sim.trace
            .record_number("restart-pending", second.pending().len());
        Ok(())
    })
}

/// A recovered attempt the journal says landed is retired, not resent.
fn restart_lands(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        run.lands()?.tick()?;
        assert_eq!(run.entered(), 1, "the first effect must have run once");

        let refused = run.lands()?.tick();
        assert!(
            !matches!(
                refused,
                Err(EcsError::Kernel(KernelError::EffectRefused { .. }))
            ),
            "a crash after the record of an effect landed must not make the run unable \
             to continue past that entry: {refused:?}"
        );
        assert_eq!(
            run.entered(),
            1,
            "the journal records the effect as landed, so nothing may send it again"
        );
        let mut second = run.lands()?;
        drop(second.tick());
        assert!(
            second.pending().is_empty(),
            "the entry is done for the generation its effect landed in: {:?}",
            second.pending()
        );
        sim.record("landed-attempt-retired");
        sim.trace
            .record_number("land-entered", u64::from(run.entered()));
        Ok(())
    })
}

/// Leave one attempt unknown, restart, and settle the recovered attempt with
/// `evidence` on the restarted bot, which is returned.
///
/// The common opening of both settlement families: they differ only in what the
/// evidence says and in what they then require of the restarted bot.
fn restart_and_settle(
    run: &Run,
    evidence: EffectEvidence,
) -> Result<Bot, Box<dyn std::error::Error>> {
    drop(run.never()?.tick());
    let key = run.first_key()?;

    let mut second = run.never()?;
    drop(second.tick());
    second.resolve_effect(&key, evidence)?;
    Ok(second)
}

/// Evidence that nothing landed makes the entry eligible again.
fn not_applied_retries(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut second = restart_and_settle(&run, EffectEvidence::NotApplied)?;
        assert_eq!(
            run.kinds()?.last(),
            Some(&EventKind::OutcomeObserved),
            "settling a recovered attempt must write the fact down; a bot that only \
             dropped the key from its own list would come back with the same unknown"
        );

        let retried = second.tick();
        assert!(
            !matches!(
                retried,
                Err(EcsError::Kernel(KernelError::EffectRefused { .. }))
            ),
            "evidence that nothing was delivered makes the entry eligible again, and the \
             next attempt must not be refused as a repeat: {retried:?}"
        );
        assert_eq!(
            run.entered(),
            2,
            "the repaired attempt has to reach the effect, not stop at the record"
        );
        sim.record("not-applied-reopens");
        sim.trace
            .record_number("retry-events", run.recorded()?.len());
        Ok(())
    })
}

/// Evidence that it landed never re-opens the entry.
fn applied_never_retries(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut second = restart_and_settle(&run, EffectEvidence::Applied)?;
        let after_settle = run.entered();

        let later = run
            .churn(sim, 4)
            .ok_or("a churn count this host cannot address")?;
        for _ in 0..later {
            drop(second.tick());
        }
        assert_eq!(run.entered(), after_settle, "a settled attempt ran again");
        sim.record("applied-never-reopens");
        sim.trace
            .record_number("applied-entered", u64::from(run.entered()));
        Ok(())
    })
}

/// A restart keeps the original key first, so recovery stays valid.
fn attempt_continues(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        drop(run.never()?.tick());
        let original = run.first_key()?;

        let mut second = run.never()?;
        drop(second.tick());
        second.resolve_effect(&original, EffectEvidence::NotApplied)?;
        drop(second.tick());

        let keys = run.distinct_keys()?;
        assert_eq!(
            keys.first().copied(),
            Some(original),
            "the first key must remain the first: a restart that renumbered history would \
             make every recovered key foreign"
        );
        sim.record("attempt-sequence-continues");
        sim.trace.record_number("attempt-keys", keys.len());
        Ok(())
    })
}

/// A held attempt does not re-enter the body on a later tick.
fn held_not_exhausted(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        drop(run.never()?.tick());

        let mut second = run.never()?;
        let held = second.tick();
        let before = run.entered();
        let later = run
            .churn(sim, 4)
            .ok_or("a churn count this host cannot address")?;
        for _ in 0..later {
            drop(second.tick());
        }
        assert_eq!(
            run.entered(),
            before,
            "a held attempt re-entered the body on a later tick, so it was not held"
        );
        sim.record("held-not-retried");
        sim.trace.record_number("held-entered", u64::from(before));
        sim.trace
            .record_number("held-tick-refused", u64::from(held.is_err()));
        sim.trace.record_number("held-later-ticks", later);
        Ok(())
    })
}

// ── The sweep ─────────────────────────────────────────────────────────────

band_family!(
restart_holds_r00 => restart_holds, 0; restart_holds_r01 => restart_holds, 1;
restart_holds_r02 => restart_holds, 2; restart_holds_r03 => restart_holds, 3;
restart_holds_r04 => restart_holds, 4; restart_holds_r05 => restart_holds, 5;
restart_holds_r06 => restart_holds, 6; restart_holds_r07 => restart_holds, 7;
restart_holds_r08 => restart_holds, 8; restart_holds_r09 => restart_holds, 9;
restart_holds_r10 => restart_holds, 10; restart_holds_r11 => restart_holds, 11;
restart_holds_r12 => restart_holds, 12; restart_holds_r13 => restart_holds, 13;
restart_holds_r14 => restart_holds, 14; restart_holds_r15 => restart_holds, 15;
restart_lands_r00 => restart_lands, 0; restart_lands_r01 => restart_lands, 1;
restart_lands_r02 => restart_lands, 2; restart_lands_r03 => restart_lands, 3;
restart_lands_r04 => restart_lands, 4; restart_lands_r05 => restart_lands, 5;
restart_lands_r06 => restart_lands, 6; restart_lands_r07 => restart_lands, 7;
restart_lands_r08 => restart_lands, 8; restart_lands_r09 => restart_lands, 9;
restart_lands_r10 => restart_lands, 10; restart_lands_r11 => restart_lands, 11;
restart_lands_r12 => restart_lands, 12; restart_lands_r13 => restart_lands, 13;
restart_lands_r14 => restart_lands, 14; restart_lands_r15 => restart_lands, 15;
not_applied_r00 => not_applied_retries, 0; not_applied_r01 => not_applied_retries, 1;
not_applied_r02 => not_applied_retries, 2; not_applied_r03 => not_applied_retries, 3;
not_applied_r04 => not_applied_retries, 4; not_applied_r05 => not_applied_retries, 5;
not_applied_r06 => not_applied_retries, 6; not_applied_r07 => not_applied_retries, 7;
not_applied_r08 => not_applied_retries, 8; not_applied_r09 => not_applied_retries, 9;
not_applied_r10 => not_applied_retries, 10; not_applied_r11 => not_applied_retries, 11;
not_applied_r12 => not_applied_retries, 12; not_applied_r13 => not_applied_retries, 13;
not_applied_r14 => not_applied_retries, 14; not_applied_r15 => not_applied_retries, 15;
applied_never_r00 => applied_never_retries, 0; applied_never_r01 => applied_never_retries, 1;
applied_never_r02 => applied_never_retries, 2; applied_never_r03 => applied_never_retries, 3;
applied_never_r04 => applied_never_retries, 4; applied_never_r05 => applied_never_retries, 5;
applied_never_r06 => applied_never_retries, 6; applied_never_r07 => applied_never_retries, 7;
applied_never_r08 => applied_never_retries, 8; applied_never_r09 => applied_never_retries, 9;
applied_never_r10 => applied_never_retries, 10; applied_never_r11 => applied_never_retries, 11;
applied_never_r12 => applied_never_retries, 12; applied_never_r13 => applied_never_retries, 13;
applied_never_r14 => applied_never_retries, 14; applied_never_r15 => applied_never_retries, 15;
attempt_continues_r00 => attempt_continues, 0; attempt_continues_r01 => attempt_continues, 1;
attempt_continues_r02 => attempt_continues, 2; attempt_continues_r03 => attempt_continues, 3;
attempt_continues_r04 => attempt_continues, 4; attempt_continues_r05 => attempt_continues, 5;
attempt_continues_r06 => attempt_continues, 6; attempt_continues_r07 => attempt_continues, 7;
attempt_continues_r08 => attempt_continues, 8; attempt_continues_r09 => attempt_continues, 9;
attempt_continues_r10 => attempt_continues, 10; attempt_continues_r11 => attempt_continues, 11;
attempt_continues_r12 => attempt_continues, 12; attempt_continues_r13 => attempt_continues, 13;
attempt_continues_r14 => attempt_continues, 14; attempt_continues_r15 => attempt_continues, 15;
held_not_exhausted_r00 => held_not_exhausted, 0; held_not_exhausted_r01 => held_not_exhausted, 1;
held_not_exhausted_r02 => held_not_exhausted, 2; held_not_exhausted_r03 => held_not_exhausted, 3;
held_not_exhausted_r04 => held_not_exhausted, 4; held_not_exhausted_r05 => held_not_exhausted, 5;
held_not_exhausted_r06 => held_not_exhausted, 6; held_not_exhausted_r07 => held_not_exhausted, 7;
held_not_exhausted_r08 => held_not_exhausted, 8; held_not_exhausted_r09 => held_not_exhausted, 9;
held_not_exhausted_r10 => held_not_exhausted, 10; held_not_exhausted_r11 => held_not_exhausted, 11;
held_not_exhausted_r12 => held_not_exhausted, 12; held_not_exhausted_r13 => held_not_exhausted, 13;
held_not_exhausted_r14 => held_not_exhausted, 14; held_not_exhausted_r15 => held_not_exhausted, 15;
);
