//! What settling and ticking again may change: pending reports, journaled
//! settlements, idempotence, and surviving unknowns.

use lgwks_bot::journal::EventKind;
use lgwks_bot::spec::EffectEvidence;

use crate::band_family::band_family;
use crate::sim::{self, Band};

use super::{Run, TestResult};

/// A pending report names the key the caller settles by.
fn pending_names_key(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut bot = run.never()?;
        drop(bot.tick());
        let action = format!("{:?}", run.first_key()?.action());

        let pending = bot.pending();
        if pending.is_empty() {
            sim.record("nothing-outstanding");
            sim.trace.record_number("pending-none", 0);
            return Ok(());
        }
        for work in &pending {
            let named = format!("{work:?}");
            assert!(
                named.contains(&action) || named.contains("UnsettledByRecovery"),
                "a pending report must name the key the caller settles by: {named}"
            );
        }
        sim.record("pending-names-key");
        sim.trace.record_number("pending", pending.len());
        Ok(())
    })
}

/// Settling writes the fact down before acknowledging it.
fn settlement_journaled(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut bot = run.never()?;
        drop(bot.tick());
        let before = run.recorded()?.len();

        bot.resolve_effect(&run.first_key()?, EffectEvidence::NotApplied)?;
        let after = run.recorded()?.len();
        assert!(
            after > before,
            "settling acknowledged a fact it never wrote down, so a restart would come \
             back with the same unknown it just cleared"
        );
        sim.record("settlement-journaled");
        sim.trace.record_number("settle-before", before);
        sim.trace.record_number("settle-after", after);
        Ok(())
    })
}

/// Settling again does not append again.
fn settle_is_idempotent(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut bot = run.never()?;
        drop(bot.tick());
        let key = run.first_key()?;

        bot.resolve_effect(&key, EffectEvidence::NotApplied)?;
        let after_first = run.recorded()?.len();
        let repeats = run
            .churn(sim, 4)
            .ok_or("a churn count this host cannot address")?;
        for _ in 0..repeats {
            drop(bot.resolve_effect(&key, EffectEvidence::NotApplied));
        }
        assert_eq!(
            run.recorded()?.len(),
            after_first,
            "settling the same key again appended another outcome"
        );
        sim.record("settle-idempotent");
        sim.trace.record_number("idempotent-events", after_first);
        sim.trace.record_number("idempotent-repeats", repeats);
        Ok(())
    })
}

/// Further ticks do not resolve a recovered unknown by omission.
fn unknown_survives(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut bot = run.never()?;
        drop(bot.tick());
        let key = run.first_key()?;
        let before = run.recorded()?.len();
        let later = run
            .churn(sim, 4)
            .ok_or("a churn count this host cannot address")?;

        for _ in 0..later {
            drop(bot.tick());
        }
        assert_eq!(
            run.recorded()?.len(),
            before,
            "ticking a bot with an unresolved effect appended to the record"
        );
        assert!(
            bot.resolve_effect(&key, EffectEvidence::Applied).is_ok(),
            "a recovered unknown must still be settleable after further ticks"
        );
        sim.record("unknown-survives-ticks");
        sim.trace
            .record_number("survive-events", run.recorded()?.len());
        sim.trace.record_number("survive-ticks", later);
        Ok(())
    })
}

/// Repeated ticks and settles never duplicate an outcome.
fn repeat_settle_safe(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut bot = run.never()?;
        drop(bot.tick());
        let key = run.first_key()?;
        let evidence = if sim.rng().chance(500) {
            EffectEvidence::NotApplied
        } else {
            EffectEvidence::Applied
        };
        bot.resolve_effect(&key, evidence)?;
        let settled = run.count(EventKind::OutcomeObserved)?;

        let churn = run
            .churn(sim, 8)
            .ok_or("a churn count this host cannot address")?;
        for _ in 0..churn {
            drop(bot.tick());
            drop(bot.resolve_effect(&key, evidence));
        }
        assert_eq!(
            run.count(EventKind::OutcomeObserved)?,
            settled,
            "churning ticks and settles after a settlement duplicated the outcome"
        );
        sim.record("repeat-churn-safe");
        sim.trace.record_number("churn-rounds", churn);
        sim.trace.record_number("churn-outcomes", settled);
        Ok(())
    })
}

// ── The sweep ─────────────────────────────────────────────────────────────

band_family!(
pending_names_key_r00 => pending_names_key, 0; pending_names_key_r01 => pending_names_key, 1;
pending_names_key_r02 => pending_names_key, 2; pending_names_key_r03 => pending_names_key, 3;
pending_names_key_r04 => pending_names_key, 4; pending_names_key_r05 => pending_names_key, 5;
pending_names_key_r06 => pending_names_key, 6; pending_names_key_r07 => pending_names_key, 7;
pending_names_key_r08 => pending_names_key, 8; pending_names_key_r09 => pending_names_key, 9;
pending_names_key_r10 => pending_names_key, 10; pending_names_key_r11 => pending_names_key, 11;
pending_names_key_r12 => pending_names_key, 12; pending_names_key_r13 => pending_names_key, 13;
pending_names_key_r14 => pending_names_key, 14; pending_names_key_r15 => pending_names_key, 15;
settlement_r00 => settlement_journaled, 0; settlement_r01 => settlement_journaled, 1;
settlement_r02 => settlement_journaled, 2; settlement_r03 => settlement_journaled, 3;
settlement_r04 => settlement_journaled, 4; settlement_r05 => settlement_journaled, 5;
settlement_r06 => settlement_journaled, 6; settlement_r07 => settlement_journaled, 7;
settlement_r08 => settlement_journaled, 8; settlement_r09 => settlement_journaled, 9;
settlement_r10 => settlement_journaled, 10; settlement_r11 => settlement_journaled, 11;
settlement_r12 => settlement_journaled, 12; settlement_r13 => settlement_journaled, 13;
settlement_r14 => settlement_journaled, 14; settlement_r15 => settlement_journaled, 15;
settle_idempotent_r00 => settle_is_idempotent, 0; settle_idempotent_r01 => settle_is_idempotent, 1;
settle_idempotent_r02 => settle_is_idempotent, 2; settle_idempotent_r03 => settle_is_idempotent, 3;
settle_idempotent_r04 => settle_is_idempotent, 4; settle_idempotent_r05 => settle_is_idempotent, 5;
settle_idempotent_r06 => settle_is_idempotent, 6; settle_idempotent_r07 => settle_is_idempotent, 7;
settle_idempotent_r08 => settle_is_idempotent, 8; settle_idempotent_r09 => settle_is_idempotent, 9;
settle_idempotent_r10 => settle_is_idempotent, 10; settle_idempotent_r11 => settle_is_idempotent, 11;
settle_idempotent_r12 => settle_is_idempotent, 12; settle_idempotent_r13 => settle_is_idempotent, 13;
settle_idempotent_r14 => settle_is_idempotent, 14; settle_idempotent_r15 => settle_is_idempotent, 15;
unknown_survives_r00 => unknown_survives, 0; unknown_survives_r01 => unknown_survives, 1;
unknown_survives_r02 => unknown_survives, 2; unknown_survives_r03 => unknown_survives, 3;
unknown_survives_r04 => unknown_survives, 4; unknown_survives_r05 => unknown_survives, 5;
unknown_survives_r06 => unknown_survives, 6; unknown_survives_r07 => unknown_survives, 7;
unknown_survives_r08 => unknown_survives, 8; unknown_survives_r09 => unknown_survives, 9;
unknown_survives_r10 => unknown_survives, 10; unknown_survives_r11 => unknown_survives, 11;
unknown_survives_r12 => unknown_survives, 12; unknown_survives_r13 => unknown_survives, 13;
unknown_survives_r14 => unknown_survives, 14; unknown_survives_r15 => unknown_survives, 15;
repeat_settle_r00 => repeat_settle_safe, 0; repeat_settle_r01 => repeat_settle_safe, 1;
repeat_settle_r02 => repeat_settle_safe, 2; repeat_settle_r03 => repeat_settle_safe, 3;
repeat_settle_r04 => repeat_settle_safe, 4; repeat_settle_r05 => repeat_settle_safe, 5;
repeat_settle_r06 => repeat_settle_safe, 6; repeat_settle_r07 => repeat_settle_safe, 7;
repeat_settle_r08 => repeat_settle_safe, 8; repeat_settle_r09 => repeat_settle_safe, 9;
repeat_settle_r10 => repeat_settle_safe, 10; repeat_settle_r11 => repeat_settle_safe, 11;
repeat_settle_r12 => repeat_settle_safe, 12; repeat_settle_r13 => repeat_settle_safe, 13;
repeat_settle_r14 => repeat_settle_safe, 14; repeat_settle_r15 => repeat_settle_safe, 15;
);
