//! The seeded families: two engines on one schedule, the receipt, the replay.

use crate::band_family;
use crate::sim::{self, Band, Rng};

use super::{TestResult, plan, record, run};

band_family::band_family! {
    receipt_band_00 => a_seed_that_changes_the_schedule_changes_the_trace, 0;
    receipt_band_01 => a_seed_that_changes_the_schedule_changes_the_trace, 1;
    receipt_band_02 => a_seed_that_changes_the_schedule_changes_the_trace, 2;
    replay_band_00 => the_same_seed_replays, 0;
    replay_band_01 => the_same_seed_replays, 1;
    replay_band_02 => the_same_seed_replays, 2;
    equivalence_band_00 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 0;
    equivalence_band_01 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 1;
    equivalence_band_02 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 2;
    equivalence_band_03 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 3;
    equivalence_band_04 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 4;
    equivalence_band_05 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 5;
    equivalence_band_06 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 6;
    equivalence_band_07 => the_change_tick_and_the_digest_fire_on_the_same_ticks, 7;
}

/// Refuse a seed on which the two engines disagreed, emitting the refusal
/// first so the scan lane's rule — every `return Err` is signalled — holds for
/// the one place this family refuses.
fn disagree(cause: String) -> TestResult {
    let refusal: TestResult = Err(cause.into());
    lgwks_std::trace::debug!(
        error = ?refusal.as_ref().err(),
        "change ticks: the two engines disagreed on a seed"
    );
    refusal
}

/// One seed, two engines, one sequence of movements.
///
/// The property the whole file exists for: the revision seam is an optimisation,
/// so it may only ever change *how long* the substrate spends deciding, never
/// what it decides.
fn the_change_tick_and_the_digest_fire_on_the_same_ticks(band: Band) -> TestResult {
    // `sim::sweep` returns the first disagreement as the family's error, with
    // the seed and both engines' records in its message, and the per-seed
    // hashes otherwise. A family that wanted every disagreement at once would
    // collect them here — but it would be reporting a *sweep* rather than a
    // property, and the first one is the one that has to be read.
    sim::sweep(band, |sim| {
        let plan = plan(sim.rng())?;
        let with_revisions = run(&plan, true)?;
        let by_value = run(&plan, false)?;
        if with_revisions.fired != by_value.fired {
            return disagree(format!(
                "seed {}: the change-tick engine fired {:?} and the \
                 value-comparison engine fired {:?} for the same movements",
                sim.seed, with_revisions.fired, by_value.fired,
            ));
        }
        if with_revisions.report != by_value.report {
            return disagree(format!(
                "seed {}: the two engines reported different rows: {:?} against {:?}",
                sim.seed, with_revisions.report, by_value.report,
            ));
        }
        // And the polls: the engines differ here by design, and the revision
        // engine may only poll *less*, never more. A revision engine that polls
        // more than the engine it replaces has bought nothing.
        let total_with: usize = with_revisions.polls.iter().sum();
        let total_without: usize = by_value.polls.iter().sum();
        if total_with > total_without {
            return disagree(format!(
                "seed {}: the change-tick engine polled {total_with} times and the \
                 value-comparison engine {total_without}, so the seam cost work",
                sim.seed,
            ));
        }
        record(sim, "rev", &with_revisions);
        record(sim, "val", &by_value);
        Ok(())
    })?;
    Ok(())
}

/// Two different schedules must not share a trace hash.
///
/// The receipt is only worth something if it distinguishes runs. A scenario where
/// every seed produced the same trace would make `the_same_seed_replays` pass for
/// a run that never varied.
fn a_seed_that_changes_the_schedule_changes_the_trace(band: Band) -> TestResult {
    let mut hashes = Vec::new();
    for seed in band.seeds().take(16) {
        let mut rng = Rng::new(seed);
        let plan = plan(&mut rng)?;
        let mut trace = sim::Trace::new();
        let log = run(&plan, true)?;
        for &(tick, chain, value) in &log.fired {
            trace.record(&format!("rev fired {tick} {chain} {value}"));
        }
        for (chain, polls) in log.polls.iter().enumerate() {
            trace.record(&format!("rev polls {chain} {polls}"));
        }
        hashes.push(trace.hash());
    }
    let mut distinct = hashes.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() > 1,
        "sixteen seeds produced {} distinct trace(s), so the hash does not distinguish \
         runs and is not a receipt: {hashes:?}",
        distinct.len()
    );
    Ok(())
}

/// The same seed, run twice, is the same run.
///
/// Everything this file claims is a claim about determinism, so a run that
/// cannot be replayed cannot be used to claim anything.
fn the_same_seed_replays(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let plan = plan(sim.rng())?;
        let with_revisions = run(&plan, true)?;
        let by_value = run(&plan, false)?;
        record(sim, "rev", &with_revisions);
        record(sim, "val", &by_value);
        Ok(())
    })
}
