//! What the dispatch writes, and when: write-ahead, outcome, ladder, isolation
//! and crash-loop families.

use std::cell::RefCell;
use std::rc::Rc;

use lgwks_bot::journal::EventKind;

use crate::band_family::band_family;
use crate::sim::{self, Band};

use super::{Run, TestResult};

/// The intent and the prepared dispatch are committed before the effect body
/// runs.
///
/// Asserted from inside the action, because that is the only vantage point
/// from which "before the handoff" means anything: a journal written after the
/// effect returned would satisfy an after-the-tick assertion just as well, and
/// would be no record of the crash this exists to survive.
fn write_ahead(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let seen = Rc::new(RefCell::new(None));

        let fired = run.notes(&seen)?.tick()?;
        assert_eq!(fired, 1, "the one chain must dispatch its one action");

        let at_handoff = seen
            .borrow()
            .clone()
            .ok_or("the action never ran, so the write-ahead claim was never measured")?;
        assert_eq!(
            at_handoff.as_slice(),
            [EventKind::IntentAdmitted, EventKind::DispatchPrepared],
            "at the instant the effect ran the journal must already hold the intent and \
             the prepared dispatch, and nothing else"
        );
        // The observation is the proof the body ran, so no counter is needed:
        // the rig's snapshotting action has no entry counter to read, and
        // inventing one here would be a second instrument measuring the same
        // fact with its own failure mode.
        sim.record("write-ahead-holds");
        sim.trace
            .record_number("write-ahead-kinds", at_handoff.len());
        Ok(())
    })
}

/// The outcome is committed after the effect, and only once.
fn outcome_after(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        run.lands()?.tick()?;

        assert_eq!(
            run.kinds()?.last(),
            Some(&EventKind::OutcomeObserved),
            "a settled dispatch must end on its recorded outcome"
        );
        assert_eq!(
            run.count(EventKind::OutcomeObserved)?,
            1,
            "one attempt settles once"
        );
        sim.record("outcome-recorded-last");
        sim.trace
            .record_number("outcome-events", run.recorded()?.len());
        Ok(())
    })
}

/// Every prepared dispatch is closed by a recorded outcome.
fn ladder_once(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let mut bot = run.lands()?;
        let ticks = run
            .churn(sim, 4)
            .ok_or("a churn count this host cannot address")?;
        for _ in 0..ticks {
            drop(bot.tick());
        }

        let prepared = run.count(EventKind::DispatchPrepared)?;
        let outcomes = run.count(EventKind::OutcomeObserved)?;
        assert_eq!(
            prepared, outcomes,
            "a prepared dispatch with no recorded outcome is a ladder that stopped halfway"
        );
        sim.record("ladder-closed");
        sim.trace.record_number("ladder-prepared", prepared);
        sim.trace.record_number("ladder-outcomes", outcomes);
        sim.trace.record_number("ladder-ticks", ticks);
        Ok(())
    })
}

/// A fresh store starts empty and records only its own attempt.
fn store_isolation(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let first = Run::new();
        let second = Run::new();
        first.lands()?.tick()?;
        let first_events = first.recorded()?.len();
        assert_eq!(
            second.recorded()?.len(),
            0,
            "a fresh store must start empty: it inherited another controller's record"
        );

        second.lands()?.tick()?;
        assert_eq!(
            second.recorded()?.len(),
            first_events,
            "the second store did not record its own attempt"
        );
        assert_eq!(
            first.recorded()?.len(),
            first_events,
            "the second controller disturbed the first store's record"
        );
        sim.record("stores-isolated");
        sim.trace.record_number("isolation-first", first_events);
        sim.trace
            .record_number("isolation-second", second.recorded()?.len());
        Ok(())
    })
}

/// Many restarts of a landed effect converge on one outcome.
fn crash_loop(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let run = Run::new();
        let cycles = run
            .churn(sim, 8)
            .ok_or("a churn count this host cannot address")?;
        for _ in 0..cycles {
            drop(run.lands()?.tick());
        }
        let outcomes = run.count(EventKind::OutcomeObserved)?;
        assert_eq!(
            outcomes, 1,
            "{cycles} restarts of a landed effect produced {outcomes} outcomes; a retry \
             loop that re-sends what already landed is the duplicate this is about"
        );
        sim.record("crash-loop-converges");
        sim.trace.record_number("crash-cycles", cycles);
        Ok(())
    })
}

// ── The sweep ─────────────────────────────────────────────────────────────

band_family!(
write_ahead_r00 => write_ahead, 0; write_ahead_r01 => write_ahead, 1;
write_ahead_r02 => write_ahead, 2; write_ahead_r03 => write_ahead, 3;
write_ahead_r04 => write_ahead, 4; write_ahead_r05 => write_ahead, 5;
write_ahead_r06 => write_ahead, 6; write_ahead_r07 => write_ahead, 7;
write_ahead_r08 => write_ahead, 8; write_ahead_r09 => write_ahead, 9;
write_ahead_r10 => write_ahead, 10; write_ahead_r11 => write_ahead, 11;
write_ahead_r12 => write_ahead, 12; write_ahead_r13 => write_ahead, 13;
write_ahead_r14 => write_ahead, 14; write_ahead_r15 => write_ahead, 15;
outcome_after_r00 => outcome_after, 0; outcome_after_r01 => outcome_after, 1;
outcome_after_r02 => outcome_after, 2; outcome_after_r03 => outcome_after, 3;
outcome_after_r04 => outcome_after, 4; outcome_after_r05 => outcome_after, 5;
outcome_after_r06 => outcome_after, 6; outcome_after_r07 => outcome_after, 7;
outcome_after_r08 => outcome_after, 8; outcome_after_r09 => outcome_after, 9;
outcome_after_r10 => outcome_after, 10; outcome_after_r11 => outcome_after, 11;
outcome_after_r12 => outcome_after, 12; outcome_after_r13 => outcome_after, 13;
outcome_after_r14 => outcome_after, 14; outcome_after_r15 => outcome_after, 15;
ladder_once_r00 => ladder_once, 0; ladder_once_r01 => ladder_once, 1;
ladder_once_r02 => ladder_once, 2; ladder_once_r03 => ladder_once, 3;
ladder_once_r04 => ladder_once, 4; ladder_once_r05 => ladder_once, 5;
ladder_once_r06 => ladder_once, 6; ladder_once_r07 => ladder_once, 7;
ladder_once_r08 => ladder_once, 8; ladder_once_r09 => ladder_once, 9;
ladder_once_r10 => ladder_once, 10; ladder_once_r11 => ladder_once, 11;
ladder_once_r12 => ladder_once, 12; ladder_once_r13 => ladder_once, 13;
ladder_once_r14 => ladder_once, 14; ladder_once_r15 => ladder_once, 15;
store_isolation_r00 => store_isolation, 0; store_isolation_r01 => store_isolation, 1;
store_isolation_r02 => store_isolation, 2; store_isolation_r03 => store_isolation, 3;
store_isolation_r04 => store_isolation, 4; store_isolation_r05 => store_isolation, 5;
store_isolation_r06 => store_isolation, 6; store_isolation_r07 => store_isolation, 7;
store_isolation_r08 => store_isolation, 8; store_isolation_r09 => store_isolation, 9;
store_isolation_r10 => store_isolation, 10; store_isolation_r11 => store_isolation, 11;
store_isolation_r12 => store_isolation, 12; store_isolation_r13 => store_isolation, 13;
store_isolation_r14 => store_isolation, 14; store_isolation_r15 => store_isolation, 15;
crash_loop_r00 => crash_loop, 0; crash_loop_r01 => crash_loop, 1;
crash_loop_r02 => crash_loop, 2; crash_loop_r03 => crash_loop, 3;
crash_loop_r04 => crash_loop, 4; crash_loop_r05 => crash_loop, 5;
crash_loop_r06 => crash_loop, 6; crash_loop_r07 => crash_loop, 7;
crash_loop_r08 => crash_loop, 8; crash_loop_r09 => crash_loop, 9;
crash_loop_r10 => crash_loop, 10; crash_loop_r11 => crash_loop, 11;
crash_loop_r12 => crash_loop, 12; crash_loop_r13 => crash_loop, 13;
crash_loop_r14 => crash_loop, 14; crash_loop_r15 => crash_loop, 15;
);
