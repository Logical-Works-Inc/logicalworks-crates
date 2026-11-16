//! T06, T07, T08: forced refreshes, a failed group beside a healthy chain, and
//! a held mass beside an independent one.

use crate::band_family::band_family;
use crate::declarable::Declarable;
use crate::sim::{self, Band};

use std::cell::RefCell;
use std::rc::Rc;

use lgwks_bot::gate::GrantSet;
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::spec::Bot;
use lgwks_bot::verb::RefreshReason;

use super::{Acting, CAUSES, Expect, TestResult, record_values, scope_of, tick_as};

/// The domain every held chain in the T06 mass reports under.
///
/// One static rather than a minted name per chain: the row is about how many held
/// chains there are, not about telling them apart by name, and a name minted per
/// chain would have to be leaked to outlive the builder that borrows it. The
/// chain index a report names comes from its position in the walk, which is
/// already distinct.
const HELD_DOMAIN: &str = "test::t06_held";

// ── T08: every declared reason forces a refresh ─────────────────────────────

/// T08: each of the four declared reasons forces the next tick to re-read the
/// source rather than settling into permanent quiet, and the forced read is
/// reported as forced with that reason.
///
/// The counterexample this closes: a source declares its baseline unsound, the
/// substrate compares the next read against the value it already holds, sees
/// *equal*, and never looks again. Zero effects fired is then indistinguishable
/// from a healthy source that held still.
fn every_declared_reason_forces_a_refresh(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        for (cause, spelling) in CAUSES {
            let forced = forced_once_by(sim, cause, spelling);
            forced?;
        }
        Ok(())
    })
}

/// One declared reason, end to end: a settled baseline, a forced re-read of an
/// unchanged value that fires nothing, and the declaration spent by that read.
fn forced_once_by(sim: &mut sim::Sim, cause: RefreshReason, spelling: &str) -> TestResult {
    let source = Declarable::new(1, "test::t08_source");
    let (_value, reason, _refusing, polls) = source.handles();
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let action = Acting::lands();
    let ran = action.record();
    let mut bot = Bot::builder("sim-t08")
        .observe(source)
        .on(|observed: &u32| *observed > 0, action)
        .with_effects(scope_of(&store)?)
        .build(&GrantSet::empty())?;

    // A clean baseline tick, so nothing is armed and nothing should be
    // forced: a bot that forced every source would satisfy this row
    // without reading anything.
    tick_as(&mut bot, Expect::Settled)?;
    let baseline = polls.get();
    assert!(
        baseline >= 1,
        "{spelling}: the baseline tick must have polled the source"
    );
    assert!(
        !bot.tick_report().forced_any(),
        "{spelling}: an undeclared source is not forced"
    );
    let fired_before = ran.borrow().len();
    assert!(
        fired_before >= 1,
        "{spelling}: the baseline tick must have fired its entry once"
    );

    // The value has not moved at all, and the baseline is declared
    // unsound. A correct implementation re-reads anyway.
    reason.set(Some(cause));
    tick_as(&mut bot, Expect::Settled)?;
    assert!(
        polls.get() > baseline,
        "{spelling}: the declared-unsound source was re-read"
    );
    assert_eq!(
        bot.tick_report()
            .forced()
            .first()
            .map(|forced| forced.reason()),
        Some(cause),
        "{spelling}: the report names the cause it forced for"
    );
    assert_eq!(
        ran.borrow().len(),
        fired_before,
        "{spelling}: a refresh of an unchanged value commits nothing and fires nothing, \
         so the repair is not 're-read forever'"
    );

    // The declaration is spent by the read it forced, so the next tick
    // has a sound baseline and is not forced.
    reason.set(None);
    tick_as(&mut bot, Expect::Settled)?;
    assert!(
        !bot.tick_report().forced_any(),
        "{spelling}: a source that declared nothing is not forced, so the previous \
         declaration was spent by the read it forced"
    );
    sim.record(spelling);
    Ok(())
}

// ── T07: one chain's failed group never rolls back another's commit ─────────

/// T07: one chain succeeds on a new fingerprint, the other chain's atomic
/// observation group is then refused twice, and after that chain recovers the
/// first chain's uncommitted update has still executed exactly once.
///
/// The counterexample this closes is a *group* failure contaminating a sibling:
/// if the substrate treated one chain's refused observation as a reason to doubt
/// every chain's, the healthy chain's newer value would be erased, duplicated or
/// silently re-fired. The row's observable is the healthy chain's run record --
/// one entry per executed update, in order, across the whole sequence.
///
/// Two facts about the substrate shape the sequence, and both are measured rather
/// than assumed. A settled chain re-fires only when its source declares a new
/// fingerprint, which is what "A succeeds with a new fingerprint" means. And a
/// refusal ends the tick before anything dispatches, so A's update lands on a
/// tick of its own rather than on the tick B fails -- which is exactly why the
/// record, not the tick, is the oracle.
fn a_failed_group_leaves_its_own_chain_forced(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let broken = Declarable::new(1, "test::t07_broken");
        let (broken_value, broken_reason, broken_refusing, broken_polls) = broken.handles();
        let healthy = Declarable::new(1, "test::t07_healthy");
        let (healthy_value, healthy_reason, _, _) = healthy.handles();
        let broken_chain = Acting::lands();
        let broken_ran = broken_chain.record();
        let healthy_chain = Acting::lands();
        let healthy_ran = healthy_chain.record();
        let store = Rc::new(RefCell::new(MemoryJournal::new()));
        let mut bot = Bot::builder("sim-t07")
            .observe(healthy)
            .on(|observed: &u32| *observed > 0, healthy_chain)
            .observe(broken)
            .on(|observed: &u32| *observed > 0, broken_chain)
            .with_effects(scope_of(&store)?)
            .build(&GrantSet::empty())?;

        tick_as(&mut bot, Expect::Settled)?;
        assert_eq!(healthy_ran.borrow().as_slice(), [1], "baseline commit");
        assert_eq!(broken_ran.borrow().as_slice(), [1], "baseline commit");

        // A succeeds on a new fingerprint.
        healthy_reason.set(Some(RefreshReason::StaleRemoteKey));
        healthy_value.set(2);
        tick_as(&mut bot, Expect::Settled)?;
        assert_eq!(
            healthy_ran.borrow().as_slice(),
            [1, 2],
            "the new fingerprint committed the newer value and fired it once"
        );

        // B's observation group is refused, and stays refused: the mark must
        // outlive the tick that made it, or B would go quiet for the same reason
        // it was quiet before.
        broken_reason.set(Some(RefreshReason::Disconnected));
        broken_refusing.set(true);
        let polls_before = broken_polls.get();
        tick_as(&mut bot, Expect::Refusal)?;
        assert!(
            broken_polls.get() > polls_before,
            "the refused chain was re-read"
        );
        tick_as(&mut bot, Expect::Refusal)?;
        assert_eq!(
            broken_ran.borrow().as_slice(),
            [1],
            "a chain whose group never landed commits nothing, however often it is re-read"
        );
        assert_eq!(
            healthy_ran.borrow().as_slice(),
            [1, 2],
            "and its sibling's update was neither erased nor duplicated by the failure: this \
             is the row's whole claim"
        );

        // B recovers, and commits exactly once.
        broken_refusing.set(false);
        broken_value.set(5);
        tick_as(&mut bot, Expect::Settled)?;
        assert_eq!(
            broken_ran.borrow().as_slice(),
            [1, 5],
            "the recovered chain executes its own update exactly once"
        );
        assert_eq!(
            healthy_ran.borrow().as_slice(),
            [1, 2],
            "and its sibling's record is unchanged"
        );

        // Subsequent changes on both chains: the first move was not a one-shot.
        healthy_reason.set(Some(RefreshReason::StaleRemoteKey));
        healthy_value.set(3);
        broken_reason.set(Some(RefreshReason::Disconnected));
        broken_value.set(6);
        tick_as(&mut bot, Expect::Settled)?;
        assert_eq!(
            healthy_ran.borrow().as_slice(),
            [1, 2, 3],
            "a subsequent new fingerprint on the healthy chain fires once"
        );
        assert_eq!(
            broken_ran.borrow().as_slice(),
            [1, 5, 6],
            "and a subsequent one on the recovered chain fires once"
        );

        record_values(sim, "broken", broken_ran.borrow().iter().copied());
        record_values(sim, "healthy", healthy_ran.borrow().iter().copied());
        Ok(())
    })
}

// ── T06: a saturated mass never starves an independent chain ────────────────

/// T06: a drawn mass of held chains, each of which enters its action once and
/// then reports itself indeterminate, never starves the independent chain
/// declared beside it.
///
/// The seed draws the mass, because "a hundred held chains" is a sample and a
/// thousand is a different claim. The oracle is a controlled completion -- a held
/// action's own reported outcome -- and never a sleep.
fn a_held_mass_never_starves_an_independent_chain(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let mass = sim.rng().between(2, 64);
        let independent = Acting::lands();
        let independent_ran = independent.record();
        let store = Rc::new(RefCell::new(MemoryJournal::new()));

        // Declared first, so it is chain 0 and its effect is the first the walk
        // reaches -- the position a starved walk would lose.
        let mut builder = Bot::builder("sim-t06")
            .observe(Declarable::new(1, "test::t06_independent"))
            .on(|observed: &u32| *observed > 0, independent);
        for _ in 0..mass {
            builder = builder
                .observe(Declarable::new(1, HELD_DOMAIN))
                .on(|observed: &u32| *observed > 0, Acting::holds());
        }
        let mut bot = builder
            .with_effects(scope_of(&store)?)
            .build(&GrantSet::empty())?;

        // The held chains each report their own indeterminate outcome. The walk
        // reports the first hold; the chains behind it still ran, because a walk
        // that stopped at the first hold would leave them unentered, which is the
        // starvation this row forbids.
        tick_as(&mut bot, Expect::Hold)?;

        assert_eq!(
            independent_ran.borrow().as_slice(),
            [1],
            "at a mass of {mass} the independent chain's action still ran"
        );
        assert_eq!(
            bot.source_domains().len(),
            usize::try_from(mass)?.saturating_add(1),
            "the bot declares one independent chain and {mass} held ones"
        );
        assert_eq!(
            bot.pending().len(),
            usize::try_from(mass)?,
            "every held chain is reported as held rather than silently dropped"
        );
        sim.trace.record_number("mass", u64::from(mass));
        Ok(())
    })
}

// ── The families, one declared test per band ───────────────────────────────

band_family! {
    every_declared_reason_forces_a_refresh_t08 => every_declared_reason_forces_a_refresh, 36;
    a_failed_group_leaves_its_own_chain_forced_t07 => a_failed_group_leaves_its_own_chain_forced, 37;
    a_held_mass_never_starves_an_independent_chain_t06 => a_held_mass_never_starves_an_independent_chain, 38;
}
