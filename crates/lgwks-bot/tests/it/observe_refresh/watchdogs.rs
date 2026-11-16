//! T06: the wave-level watchdog spends one thread per wedged wave, and none on
//! an ordinary tick.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::time::Duration;

use lgwks_bot::gate::GrantSet;
use lgwks_bot::spec::Bot;

use crate::effects::memory_scope;

use super::{Lands, TestResult, Wedged};

/// A wave spends one watchdog, a mass of waves spends one per wedged wave, and
/// an ordinary tick spends none.
///
/// The row the wave-level watchdog exists for, at the two scales a fixed
/// chain-count fixture cannot reach. The bot declared here is
/// [`MAX_WAVE_CHAINS`] chains wide, so it is exactly **one** observation wave,
/// and it is declared a second time over [`MASS_CHAINS`] chains so the number of
/// waves is larger than one. The source that never resolves is chain 0 in both,
/// which is the first wave.
///
/// The assertion is on `TickReport::watchdogs`, the number an operator reads, not
/// on a test-only counter: a thread count the product cannot report is a thread
/// count the product has promised nothing about, and a test that read a private
/// field would be measuring its own instrumentation.
///
/// Four observations, each a different way to get this wrong:
///
/// 1. a wave with one wedged source spends exactly **one** thread — not one per
///    chain, which is the defect this replaced;
/// 2. the same wave with nothing wedged spends **zero**, which is the lazy half:
///    a poll that answers on its first poll never needed watching;
/// 3. a bot of several waves with one wedged source still spends **one**, because
///    the waves that resolved had nothing pending;
/// 4. every source that was given up on is still **reported**, so a watchdog that
///    was simply never started cannot hide a stall by never firing.
#[test]
fn a_wave_spends_one_watchdog_and_a_mass_of_waves_spends_one_each() -> TestResult {
    // One wave, one wedged source among thirty-one ready ones.
    let mut stalled = mass_bot(
        "t06-watchdog-wave",
        MAX_WAVE_CHAINS,
        true,
        WATCHDOG_DEADLINE,
    )?;
    let _outcome = stalled.tick();
    let report = stalled.tick_report();
    assert_eq!(
        report.watchdogs(),
        1,
        "a wave of {} chains with one source that never resolved spends exactly one \
         watchdog thread, not one per chain",
        MAX_WAVE_CHAINS
    );
    assert_eq!(
        report.stalled().len(),
        1,
        "and reports exactly the one source that stopped answering: {:?}",
        report.stalled()
    );

    // The same wave with nothing wedged: every source answers on its first poll,
    // so there is nothing to watch and the tick spends no thread at all.
    let mut ready = mass_bot(
        "t06-watchdog-ready",
        MAX_WAVE_CHAINS,
        false,
        WATCHDOG_DEADLINE,
    )?;
    let _outcome = ready.tick();
    assert_eq!(
        ready.tick_report().watchdogs(),
        0,
        "an ordinary tick — every source answered on its first poll — starts no \
         deadline thread at all"
    );
    assert!(
        !ready.tick_report().stalled_any(),
        "and reports no stall, because nothing was given up on"
    );

    // Several waves, one wedged source: only the wave holding it is watched.
    let mut mass = mass_bot("t06-watchdog-mass", MASS_CHAINS, true, WATCHDOG_DEADLINE)?;
    let _outcome = mass.tick();
    let mass_report = mass.tick_report();
    assert_eq!(
        mass_report.watchdogs(),
        1,
        "a bot of {MASS_CHAINS} chains is {} waves, and only the one holding the \
         wedged source spends a thread",
        mass_chains(MASS_CHAINS)
    );
    assert_eq!(
        mass_report.stalled().len(),
        1,
        "and the same one chain is reported, whatever the number of waves: {:?}",
        mass_report.stalled()
    );
    Ok(())
}

/// The budget the watchdog fixtures run under.
///
/// Short because this test's subject is a *count* and not a duration: it waits
/// for two real expiries (one wave and one mass) and for nothing else, and the
/// declared thirty-second default would make the count take a minute to observe.
/// Whether a wave spends a thread does not depend on the budget's size, only on
/// whether the reaper had to start at all.
const WATCHDOG_DEADLINE: Duration = Duration::from_millis(120);

/// The chain count one observation wave holds.
///
/// A copy of the substrate's own fan-out rather than a re-derivation: it is the
/// number a wave in *this* fixture spans, and the substrate's constant is the
/// authority on what a wave is. A test that derived it would pass against a
/// substrate whose fan-out had changed, because it would have followed.
const MAX_WAVE_CHAINS: usize = 32;

/// How many chains the multi-wave fixture declares.
///
/// Three full waves plus a remainder, so the number of waves is not a round
/// figure and an assertion that divided by the wave width exactly would miss the
/// fourth wave entirely.
const MASS_CHAINS: usize = 100;

/// How many observation waves a bot of `chains` chains polls in.
fn mass_chains(chains: usize) -> usize {
    chains.div_ceil(MAX_WAVE_CHAINS)
}

/// Declare one more always-answering chain on `$name`, which must already hold a
/// source.
///
/// A macro rather than a loop in the call site because the builder's type follows
/// its source, so a runtime chain count cannot be expressed at all; a macro is
/// the one form in which `n` chains is `n` declarations without `n` copies of the
/// same block for the repository's repetition check to find.
macro_rules! another_chain {
    ($name:ident) => {
        $name = $name
            .observe(Wedged {
                wedged: Rc::new(Cell::new(false)),
                value: Rc::new(Cell::new(1)),
                entered: Rc::new(Cell::new(0)),
                reason: Rc::new(Cell::new(None)),
                domain: "test::mass",
            })
            .on(
                |observed: &u32| *observed > 0,
                Lands {
                    seen: Rc::new(RefCell::new(Vec::new())),
                },
            );
    };
}

/// A bot of `chains` chains whose first source parks forever when `wedged`.
///
/// Every chain after the first is always-ready by construction, which is what
/// makes the count exact: only the first wave can be pending at all, and it is
/// pending only because of chain 0.
fn mass_bot(
    name: &'static str,
    chains: usize,
    wedged: bool,
    deadline: Duration,
) -> Result<Bot, Box<dyn Error>> {
    let mut builder = Bot::builder(name)
        .with_poll_deadline(deadline)
        .observe(Wedged {
            wedged: Rc::new(Cell::new(wedged)),
            value: Rc::new(Cell::new(1)),
            entered: Rc::new(Cell::new(0)),
            reason: Rc::new(Cell::new(None)),
            domain: "test::mass-wedged",
        })
        .on(
            |observed: &u32| *observed > 0,
            Lands {
                seen: Rc::new(RefCell::new(Vec::new())),
            },
        );
    for _ in 1..chains {
        another_chain!(builder);
    }
    Ok(builder
        .with_effects(memory_scope()?)
        .build(&GrantSet::empty())?)
}
